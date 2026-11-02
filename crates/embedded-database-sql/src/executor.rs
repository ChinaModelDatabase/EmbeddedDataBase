//! SQL 执行器。将 AST 绑定到 KvDb 执行。

use crate::ast::*;
use crate::function::BuiltinAggregates;
use crate::lexer::tokenize;
use crate::parser::parse;
use crate::result::{QueryResult, SqlError, Value};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use embedded_database_kv::KvDb;

/// 执行 SQL 语句。
pub fn execute(sql: &str, db: &mut KvDb) -> Result<QueryResult, SqlError> {
    let tokens = tokenize(sql).map_err(SqlError::Lex)?;
    let stmt = parse(&tokens).map_err(SqlError::Parse)?;
    match stmt {
        Statement::Insert(s) => execute_insert(s, db),
        Statement::Select(s) => execute_select(s, db),
        Statement::CreateTable(_) => Ok(QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
        }),
        Statement::DropTable(_) => Ok(QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
        }),
    }
}

/// 执行 INSERT。
///
/// 数据 key 为 `{table}:{first_value}`，payload 为 `col1=val1|col2=val2`。
/// 另写入行索引 `{table}:__idx`（值为 `|` 分隔的 first_value 列表），
/// 供 SELECT 在无范围扫描接口时按 key 逐条 `get`。
fn execute_insert(stmt: InsertStmt, db: &mut KvDb) -> Result<QueryResult, SqlError> {
    for row in &stmt.values {
        let Some(first_expr) = row.first() else {
            continue;
        };
        let first_val = expr_to_string(first_expr);
        let mut parts = Vec::new();
        for (i, val) in row.iter().enumerate() {
            let col = stmt.columns.get(i).map_or("v", |s| s.as_str());
            let v = expr_to_string(val);
            parts.push(format!("{col}={v}"));
        }
        let payload = parts.join("|");
        let key = format!("{}:{}", stmt.table, first_val);
        db.put(key.as_bytes(), payload.as_bytes())
            .map_err(|e| SqlError::Db(format!("{e:?}")))?;

        let index_key = format!("{}:__idx", stmt.table);
        let mut entries: Vec<String> = match db.get(index_key.as_bytes()) {
            Ok(Some(bytes)) => String::from_utf8_lossy(&bytes)
                .split('|')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect(),
            Ok(None) => Vec::new(),
            Err(e) => return Err(SqlError::Db(format!("{e:?}"))),
        };
        if !entries.iter().any(|e| e == &first_val) {
            entries.push(first_val);
            let index_val = entries.join("|");
            db.put(index_key.as_bytes(), index_val.as_bytes())
                .map_err(|e| SqlError::Db(format!("{e:?}")))?;
        }
    }
    Ok(QueryResult {
        columns: Vec::new(),
        rows: Vec::new(),
    })
}

/// 执行 SELECT。
///
/// M19：用 `KvDb::scan` 按 `{table}:` 前缀取行，WHERE 过滤基于 payload 解析。
/// 扫描结果中的 `{table}:__idx` 为 INSERT 行索引，不是数据行，需跳过。
fn execute_select(stmt: SelectStmt, db: &mut KvDb) -> Result<QueryResult, SqlError> {
    let table = stmt.from.as_deref().unwrap_or("");
    let prefix = format!("{table}:");
    let scan_results = db
        .scan(&prefix)
        .map_err(|e| SqlError::Db(format!("{e:?}")))?;

    let want_all = stmt
        .columns
        .iter()
        .any(|c| matches!(c, SelectItem::Wildcard));

    let mut columns: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<Literal>> = Vec::new();

    for (key, payload) in &scan_results {
        if key.ends_with(":__idx") {
            continue;
        }
        let payload = String::from_utf8_lossy(payload);
        let (row_cols, row_vals) = parse_payload(&payload);
        if let Some(condition) = &stmt.where_clause {
            if !evaluate_condition(condition, &row_cols, &row_vals) {
                continue;
            }
        }
        if columns.is_empty() {
            columns = row_cols;
        }
        rows.push(row_vals);
    }

    // JOIN（M23：嵌套循环连接，ON 仅支持 `=` 等值）
    for join in &stmt.joins {
        let right_prefix = format!("{}:", join.table);
        let right_results = db
            .scan(&right_prefix)
            .map_err(|e| SqlError::Db(format!("{e:?}")))?;
        let mut right_rows: Vec<(Vec<String>, Vec<Literal>)> = Vec::new();
        for (key, payload) in &right_results {
            if key.ends_with(":__idx") {
                continue;
            }
            let payload = String::from_utf8_lossy(payload);
            right_rows.push(parse_payload(&payload));
        }

        let left_col = col_key(&join.on_left);
        let right_col = col_key(&join.on_right);
        let left_idx = columns.iter().position(|c| c == left_col);
        let base_count = columns.len();

        // 右表独有列并入 columns（同名列保留左表列）
        for (r_cols, _) in &right_rows {
            for c in r_cols {
                if !columns.contains(c) {
                    columns.push(c.clone());
                }
            }
        }

        let mut joined_rows = Vec::new();
        for left_row in &rows {
            for (r_cols, r_vals) in &right_rows {
                let matched = match (left_idx, r_cols.iter().position(|c| c == right_col)) {
                    (Some(li), Some(ri)) => {
                        let lv = left_row.get(li).map(literal_to_string).unwrap_or_default();
                        let rv = r_vals.get(ri).map(literal_to_string).unwrap_or_default();
                        lv == rv
                    }
                    _ => false,
                };
                if !matched {
                    continue;
                }
                let mut combined = left_row.clone();
                for col_name in columns.iter().skip(base_count) {
                    let lit = r_cols
                        .iter()
                        .position(|c| c == col_name)
                        .and_then(|i| r_vals.get(i))
                        .cloned()
                        .unwrap_or(Literal::Null);
                    combined.push(lit);
                }
                joined_rows.push(combined);
            }
        }
        rows = joined_rows;
    }

    // GROUP BY 分组聚合（M25）：先按分组列分组，每组独立计算聚合
    let has_group_by = !stmt.group_by.is_empty();
    let has_aggregate = stmt
        .columns
        .iter()
        .any(|c| matches!(c, SelectItem::Aggregate(_)));

    // HAVING 过滤（M26/M27）：在聚合折叠前、基于原始分组行计算。
    // M27：不依赖 stmt.columns 查找聚合列下标，直接从 having 表达式
    // 提取聚合函数并按分组计算。
    if let Some(Expr::BinaryOp(lhs, op, rhs)) = &stmt.having {
        let agg_fn = match lhs.as_ref() {
            Expr::Aggregate(f) => Some(*f),
            _ => None,
        };
        let threshold = match rhs.as_ref() {
            Expr::Literal(Literal::Integer(v)) => Some(*v as f64),
            Expr::Literal(Literal::Float(v)) => Some(*v),
            _ => None,
        };
        if let (Some(agg_fn), Some(threshold)) = (agg_fn, threshold) {
            if has_group_by {
                let group_col = stmt.group_by.first().map(|s| col_key(s)).unwrap_or("");
                let group_idx = columns.iter().position(|c| c == group_col);
                let mut groups: Vec<(String, Vec<Vec<Literal>>)> = Vec::new();
                for row in &rows {
                    let group_val = match group_idx {
                        Some(idx) => row.get(idx).map(literal_to_string).unwrap_or_default(),
                        None => String::new(),
                    };
                    if let Some((_, grow)) = groups.iter_mut().find(|(gval, _)| *gval == group_val)
                    {
                        grow.push(row.clone());
                    } else {
                        groups.push((group_val, vec![row.clone()]));
                    }
                }
                let mut kept: Vec<Vec<Literal>> = Vec::new();
                for (group_val, group_rows) in &groups {
                    let agg_val = compute_aggregate_value(agg_fn, group_rows);
                    if !having_passes(&agg_val, op, threshold) {
                        continue;
                    }
                    if has_aggregate {
                        kept.extend(group_rows.iter().cloned());
                    } else {
                        // SELECT 列表无聚合列：每个存活组折叠为一行（组键列）
                        let mut out_row = vec![Literal::Null; columns.len()];
                        if let Some(idx) = group_idx {
                            out_row[idx] = Literal::StringLit(group_val.clone());
                        }
                        kept.push(out_row);
                    }
                }
                rows = kept;
            } else {
                // 非分组：对全部行计算一次聚合，不通过则清空
                let agg_val = compute_aggregate_value(agg_fn, &rows);
                if !having_passes(&agg_val, op, threshold) {
                    rows.clear();
                }
            }
        }
    }

    if has_group_by && has_aggregate {
        let group_col = stmt.group_by.first().map(|s| col_key(s)).unwrap_or("");
        let group_idx = columns.iter().position(|c| c == group_col);
        let mut groups: Vec<(String, Vec<Vec<Literal>>)> = Vec::new();
        for row in &rows {
            // 分组列不存在时所有行归为一组（M25 简化）
            let group_val = match group_idx {
                Some(idx) => row.get(idx).map(literal_to_string).unwrap_or_default(),
                None => String::new(),
            };
            if let Some((_, grow)) = groups.iter_mut().find(|(gval, _)| *gval == group_val) {
                grow.push(row.clone());
            } else {
                groups.push((group_val, vec![row.clone()]));
            }
        }
        let mut result_rows: Vec<Vec<Literal>> = Vec::new();
        for (group_val, group_rows) in &groups {
            let mut agg_row: Vec<Literal> = Vec::new();
            for item in &stmt.columns {
                match item {
                    SelectItem::Aggregate(f) => {
                        agg_row.push(compute_aggregate_value(*f, group_rows));
                    }
                    _ => agg_row.push(Literal::StringLit(group_val.clone())),
                }
            }
            result_rows.push(agg_row);
        }
        rows = result_rows;
    } else if has_aggregate {
        // 聚合函数（M24）：WHERE/JOIN 过滤后，将多行折叠为单行聚合结果
        let mut agg_result: Vec<Literal> = Vec::new();
        for item in &stmt.columns {
            if let SelectItem::Aggregate(f) = item {
                agg_result.push(compute_aggregate_value(*f, &rows));
            }
        }
        rows = vec![agg_result];
    }

    // ORDER BY（多列，M22：逐列字符串比较）
    if !stmt.order_by.is_empty() {
        rows.sort_by(|a, b| {
            for order in &stmt.order_by {
                let col_name = match &order.expr {
                    Expr::Column(name) => col_key(name),
                    _ => "",
                };
                let a_val = columns
                    .iter()
                    .position(|c| c == col_name)
                    .and_then(|idx| a.get(idx))
                    .map(literal_to_string)
                    .unwrap_or_default();
                let b_val = columns
                    .iter()
                    .position(|c| c == col_name)
                    .and_then(|idx| b.get(idx))
                    .map(literal_to_string)
                    .unwrap_or_default();
                let cmp = if order.asc {
                    a_val.cmp(&b_val)
                } else {
                    b_val.cmp(&a_val)
                };
                if cmp != core::cmp::Ordering::Equal {
                    return cmp;
                }
            }
            core::cmp::Ordering::Equal
        });
    }

    // LIMIT
    if let Some(n) = stmt.limit {
        let n = n as usize;
        if rows.len() > n {
            rows.truncate(n);
        }
    }

    if want_all {
        return Ok(QueryResult { columns, rows });
    }

    let proj_cols: Vec<String> = stmt
        .columns
        .iter()
        .filter_map(|c| match c {
            SelectItem::Column(name) => Some(name.clone()),
            SelectItem::Wildcard | SelectItem::Aggregate(_) => None,
        })
        .collect();

    let mut proj_rows = Vec::new();
    for row in &rows {
        let mut projected = Vec::new();
        for col in &proj_cols {
            let lit = columns
                .iter()
                .position(|c| c == col_key(col))
                .and_then(|idx| row.get(idx))
                .cloned()
                .unwrap_or(Literal::Null);
            projected.push(lit);
        }
        proj_rows.push(projected);
    }

    Ok(QueryResult {
        columns: proj_cols,
        rows: proj_rows,
    })
}

/// 列名取 `.` 后最后一段（`users.name` → `name`）。
fn col_key(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((_, last)) => last,
        None => name,
    }
}

/// 解析 `col=val|col=val` payload。
fn parse_payload(payload: &str) -> (Vec<String>, Vec<Literal>) {
    let mut cols = Vec::new();
    let mut vals = Vec::new();
    for part in payload.split('|') {
        if part.is_empty() {
            continue;
        }
        if let Some((c, v)) = part.split_once('=') {
            cols.push(String::from(c));
            vals.push(string_to_literal(v));
        }
    }
    (cols, vals)
}

/// 字符串还原为字面量。
fn string_to_literal(s: &str) -> Literal {
    if s == "NULL" {
        return Literal::Null;
    }
    if let Ok(v) = s.parse::<i64>() {
        return Literal::Integer(v);
    }
    if let Ok(v) = s.parse::<f64>() {
        return Literal::Float(v);
    }
    Literal::StringLit(String::from(s))
}

/// 表达式转字符串（VALUES 中解析器只产生字面量）。
fn expr_to_string(expr: &Expr) -> String {
    match expr {
        Expr::Literal(lit) => literal_to_string(lit),
        Expr::Column(name) => name.clone(),
        Expr::IsNull(..) => "NULL".to_string(),
        _ => String::new(),
    }
}

/// 字面量转字符串。
fn literal_to_string(lit: &Literal) -> String {
    match lit {
        Literal::Integer(v) => alloc::format!("{v}"),
        Literal::Float(v) => alloc::format!("{v}"),
        Literal::StringLit(s) => s.clone(),
        Literal::Null => "NULL".to_string(),
    }
}

/// 字面量转 f64（聚合计算用）。
fn literal_to_f64(lit: &Literal) -> Option<f64> {
    match lit {
        Literal::Integer(v) => Some(*v as f64),
        Literal::Float(v) => Some(*v),
        Literal::StringLit(s) => s.parse::<f64>().ok(),
        Literal::Null => None,
    }
}

/// 计算聚合函数值（HAVING 与 GROUP BY 聚合共用）。
/// 通过 `BuiltinAggregates` 分发：取每行第一列作为聚合输入。
fn compute_aggregate_value(agg_fn: AggregateFn, rows: &[Vec<Literal>]) -> Literal {
    let name = match agg_fn {
        AggregateFn::Count => "count",
        AggregateFn::Sum => "sum",
        AggregateFn::Avg => "avg",
        AggregateFn::Min => "min",
        AggregateFn::Max => "max",
    };
    let Some(func) = BuiltinAggregates::get(name) else {
        return Literal::Null;
    };
    let mut state = func.init();
    for row in rows {
        let arg = row.first().map(literal_to_value).unwrap_or(Value::Null);
        if func.step(&mut state, arg).is_err() {
            return Literal::Null;
        }
    }
    value_to_literal(&func.finalize(&state))
}

/// 字面量转函数运行时值。
fn literal_to_value(lit: &Literal) -> Value {
    match lit {
        Literal::Integer(v) => Value::Int(*v),
        Literal::Float(v) => Value::Float(*v),
        Literal::StringLit(s) => Value::Text(s.clone()),
        Literal::Null => Value::Null,
    }
}

/// 函数运行时值转字面量。
fn value_to_literal(value: &Value) -> Literal {
    match value {
        Value::Int(v) => Literal::Integer(*v),
        Value::Float(v) => Literal::Float(*v),
        Value::Text(s) => Literal::StringLit(s.clone()),
        Value::Boolean(b) => Literal::Integer(if *b { 1 } else { 0 }),
        Value::Blob(bytes) => {
            Literal::StringLit(alloc::string::String::from_utf8_lossy(bytes).into_owned())
        }
        Value::Null => Literal::Null,
    }
}

/// HAVING 条件：聚合值与阈值按比较运算符判断是否保留。
fn having_passes(agg_val: &Literal, op: &BinOp, threshold: f64) -> bool {
    let Some(val) = literal_to_f64(agg_val) else {
        return true;
    };
    match op {
        BinOp::Gt => val > threshold,
        BinOp::Ge => val >= threshold,
        BinOp::Lt => val < threshold,
        BinOp::Le => val <= threshold,
        BinOp::Eq => val == threshold,
        BinOp::Ne => val != threshold,
        _ => true,
    }
}

/// 评估 WHERE 条件。列以 `cols`/`vals` 平行数组表示。
fn evaluate_condition(cond: &Expr, cols: &[String], vals: &[Literal]) -> bool {
    match cond {
        Expr::BinaryOp(lhs, op, rhs) => {
            let lhs_val = evaluate_expr(lhs, cols, vals);
            let rhs_val = evaluate_expr(rhs, cols, vals);
            match op {
                BinOp::Eq => lhs_val == rhs_val,
                BinOp::Ne => lhs_val != rhs_val,
                BinOp::Lt => lhs_val < rhs_val,
                BinOp::Le => lhs_val <= rhs_val,
                BinOp::Gt => lhs_val > rhs_val,
                BinOp::Ge => lhs_val >= rhs_val,
                _ => true,
            }
        }
        _ => true,
    }
}

/// 求值表达式（列查找或字面量转字符串）。
fn evaluate_expr(expr: &Expr, cols: &[String], vals: &[Literal]) -> String {
    match expr {
        Expr::Column(name) => cols
            .iter()
            .position(|c| c == name)
            .and_then(|idx| vals.get(idx))
            .map(literal_to_string)
            .unwrap_or_default(),
        Expr::Literal(lit) => literal_to_string(lit),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::Literal;
    use crate::executor::execute;
    use alloc::string::{String, ToString};
    use alloc::vec;
    use embedded_database_core::{Config, InMemoryDevice};
    use embedded_database_kv::KvDb;

    fn make_db() -> InMemoryDevice {
        InMemoryDevice::new(1 << 20)
    }

    #[test]
    fn 执行_insert_写入() {
        let mut dev = make_db();
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        let result = execute("INSERT INTO users (name) VALUES ('alice')", &mut kv).unwrap();
        assert!(result.rows.is_empty());
    }

    #[test]
    fn 执行_select_返回结果() {
        let mut dev = make_db();
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        execute("INSERT INTO users (name) VALUES ('alice')", &mut kv).unwrap();
        let result = execute("SELECT * FROM users", &mut kv).unwrap();
        assert_eq!(result.columns, vec!["name".to_string()]);
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn 执行_空表_select返回空() {
        let mut dev = make_db();
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        let result = execute("SELECT * FROM users", &mut kv).unwrap();
        assert!(result.rows.is_empty());
    }

    #[test]
    fn 执行_非法sql报错() {
        let mut dev = make_db();
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        assert!(execute("FOO BAR", &mut kv).is_err());
    }

    #[test]
    fn 执行_select_where_过滤() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        execute(
            "INSERT INTO users (name, age) VALUES ('alice', 30)",
            &mut kv,
        )
        .unwrap();
        execute("INSERT INTO users (name, age) VALUES ('bob', 25)", &mut kv).unwrap();
        execute(
            "INSERT INTO users (name, age) VALUES ('carol', 35)",
            &mut kv,
        )
        .unwrap();
        let result = execute("SELECT name FROM users WHERE age = 30", &mut kv).unwrap();
        assert_eq!(result.rows.len(), 1);
        // M19 简化：WHERE 过滤基于 payload 解析
    }

    #[test]
    fn 执行_select_where_无匹配() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        execute("INSERT INTO users (name) VALUES ('alice')", &mut kv).unwrap();
        let result = execute("SELECT name FROM users WHERE name = 'nonexistent'", &mut kv).unwrap();
        assert!(result.rows.is_empty());
    }

    #[test]
    fn 执行_select_order_by_排序() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        execute(
            "INSERT INTO users (name, age) VALUES ('alice', 30)",
            &mut kv,
        )
        .unwrap();
        execute("INSERT INTO users (name, age) VALUES ('bob', 25)", &mut kv).unwrap();
        execute(
            "INSERT INTO users (name, age) VALUES ('carol', 35)",
            &mut kv,
        )
        .unwrap();
        let result = execute("SELECT name FROM users ORDER BY name ASC", &mut kv).unwrap();
        assert_eq!(result.rows.len(), 3);
        // M21 简化：排序基于字符串比较
    }

    #[test]
    fn 执行_select_limit_截断() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        execute("INSERT INTO users (name) VALUES ('a')", &mut kv).unwrap();
        execute("INSERT INTO users (name) VALUES ('b')", &mut kv).unwrap();
        execute("INSERT INTO users (name) VALUES ('c')", &mut kv).unwrap();
        let result = execute("SELECT name FROM users LIMIT 2", &mut kv).unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn 执行_select_order_by_desc() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        execute("INSERT INTO users (name) VALUES ('a')", &mut kv).unwrap();
        execute("INSERT INTO users (name) VALUES ('b')", &mut kv).unwrap();
        let result = execute("SELECT name FROM users ORDER BY name DESC", &mut kv).unwrap();
        assert_eq!(result.rows.len(), 2);
        // M21 简化：降序排列
    }

    #[test]
    fn 执行_select_order_by_多列() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        execute(
            "INSERT INTO users (name, age) VALUES ('alice', 25)",
            &mut kv,
        )
        .unwrap();
        execute(
            "INSERT INTO users (name, age) VALUES ('alice', 30)",
            &mut kv,
        )
        .unwrap();
        execute("INSERT INTO users (name, age) VALUES ('bob', 30)", &mut kv).unwrap();
        let result = execute(
            "SELECT name, age FROM users ORDER BY name ASC, age DESC",
            &mut kv,
        )
        .unwrap();
        assert_eq!(result.rows.len(), 3);
        // M22：多列排序基于字符串比较——alice 组内 age 降序
        assert_eq!(
            result.rows[0],
            vec![
                Literal::StringLit(String::from("alice")),
                Literal::Integer(30)
            ]
        );
        assert_eq!(
            result.rows[1],
            vec![
                Literal::StringLit(String::from("alice")),
                Literal::Integer(25)
            ]
        );
        assert_eq!(
            result.rows[2],
            vec![
                Literal::StringLit(String::from("bob")),
                Literal::Integer(30)
            ]
        );
    }

    #[test]
    fn 执行_select_join_嵌套循环() {
        // 简报用 Config::default()（max_pages=8）+ 1MiB 设备跑 5 次 INSERT
        // （每次 2 个 put/flush）会超出单段页预算 → InvalidArgument。
        // 已批准修正：加大设备与 max_pages。
        let mut dev = InMemoryDevice::new(4 << 20);
        let cfg = Config {
            max_pages: 32,
            ..Config::default()
        };
        let mut kv = KvDb::open(&mut dev, cfg).unwrap();
        execute("INSERT INTO users (id, name) VALUES (1, 'alice')", &mut kv).unwrap();
        execute("INSERT INTO users (id, name) VALUES (2, 'bob')", &mut kv).unwrap();
        execute(
            "INSERT INTO orders (user_id, item) VALUES (1, 'book')",
            &mut kv,
        )
        .unwrap();
        execute(
            "INSERT INTO orders (user_id, item) VALUES (2, 'pen')",
            &mut kv,
        )
        .unwrap();
        execute(
            "INSERT INTO orders (user_id, item) VALUES (3, 'lamp')",
            &mut kv,
        )
        .unwrap();
        let result = execute(
            "SELECT users.name FROM users JOIN orders ON users.id = orders.user_id",
            &mut kv,
        )
        .unwrap();
        // M23 简化：嵌套循环连接，匹配行数 2（id=1 和 id=2）
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn 执行_select_join_无匹配() {
        let mut dev = InMemoryDevice::new(4 << 20);
        let cfg = Config {
            max_pages: 32,
            ..Config::default()
        };
        let mut kv = KvDb::open(&mut dev, cfg).unwrap();
        execute("INSERT INTO users (id, name) VALUES (1, 'alice')", &mut kv).unwrap();
        execute(
            "INSERT INTO orders (user_id, item) VALUES (99, 'lamp')",
            &mut kv,
        )
        .unwrap();
        let result = execute(
            "SELECT users.name FROM users JOIN orders ON users.id = orders.user_id",
            &mut kv,
        )
        .unwrap();
        assert!(result.rows.is_empty());
    }

    #[test]
    fn 执行_select_count() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        execute("INSERT INTO users (name) VALUES ('a')", &mut kv).unwrap();
        execute("INSERT INTO users (name) VALUES ('b')", &mut kv).unwrap();
        execute("INSERT INTO users (name) VALUES ('c')", &mut kv).unwrap();
        let result = execute("SELECT COUNT(*) FROM users", &mut kv).unwrap();
        assert_eq!(result.rows.len(), 1);
        // M24 简化：COUNT 结果为行数
    }

    #[test]
    fn 执行_select_sum() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        execute("INSERT INTO users (age) VALUES (10)", &mut kv).unwrap();
        execute("INSERT INTO users (age) VALUES (20)", &mut kv).unwrap();
        let result = execute("SELECT SUM(age) FROM users", &mut kv).unwrap();
        assert_eq!(result.rows.len(), 1);
        // M24 简化：SUM 结果为数值和
    }

    #[test]
    fn 执行_select_empty_count() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        let result = execute("SELECT COUNT(*) FROM empty", &mut kv).unwrap();
        assert_eq!(result.rows.len(), 1);
        // M24 简化：空表 COUNT 返回 0
    }

    #[test]
    fn 执行_select_group_by_分组计数() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        execute(
            "INSERT INTO users (dept, name) VALUES ('eng', 'alice')",
            &mut kv,
        )
        .unwrap();
        execute(
            "INSERT INTO users (dept, name) VALUES ('eng', 'bob')",
            &mut kv,
        )
        .unwrap();
        execute(
            "INSERT INTO users (dept, name) VALUES ('hr', 'carol')",
            &mut kv,
        )
        .unwrap();
        let result = execute("SELECT dept, COUNT(*) FROM users GROUP BY dept", &mut kv).unwrap();
        // M25 简化：分组聚合基于字符串比较
        assert!(!result.rows.is_empty());
    }

    #[test]
    fn 执行_select_group_by_空组() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        execute(
            "INSERT INTO users (dept, name) VALUES ('eng', 'alice')",
            &mut kv,
        )
        .unwrap();
        let result = execute("SELECT dept, COUNT(*) FROM users GROUP BY missing", &mut kv).unwrap();
        // M25 简化：分组列不存在时所有行归为一组
        assert!(!result.rows.is_empty());
    }

    #[test]
    fn 执行_select_having_过滤() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        execute(
            "INSERT INTO users (dept, name) VALUES ('eng', 'alice')",
            &mut kv,
        )
        .unwrap();
        execute(
            "INSERT INTO users (dept, name) VALUES ('eng', 'bob')",
            &mut kv,
        )
        .unwrap();
        execute(
            "INSERT INTO users (dept, name) VALUES ('hr', 'carol')",
            &mut kv,
        )
        .unwrap();
        let result = execute(
            "SELECT dept, COUNT(*) FROM users GROUP BY dept HAVING COUNT(*) > 1",
            &mut kv,
        )
        .unwrap();
        // M26 简化：HAVING 过滤基于字符串比较
        // eng 组有 2 行（>1），hr 组有 1 行（<=1）
        // 过滤后应只剩 eng 组
        assert!(!result.rows.is_empty());
    }

    #[test]
    fn 执行_select_having_无匹配() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        execute(
            "INSERT INTO users (dept, name) VALUES ('eng', 'alice')",
            &mut kv,
        )
        .unwrap();
        let result = execute(
            "SELECT dept, COUNT(*) FROM users GROUP BY dept HAVING COUNT(*) > 100",
            &mut kv,
        )
        .unwrap();
        // eng 组只有 1 行（<=100），过滤后应为空
        assert!(result.rows.is_empty());
    }

    #[test]
    fn 执行_select_having_聚合列不在select列表() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        execute(
            "INSERT INTO users (dept, name) VALUES ('eng', 'alice')",
            &mut kv,
        )
        .unwrap();
        execute(
            "INSERT INTO users (dept, name) VALUES ('eng', 'bob')",
            &mut kv,
        )
        .unwrap();
        // HAVING 引用 COUNT(*)，但 SELECT 列表只有 dept
        let result = execute(
            "SELECT dept FROM users GROUP BY dept HAVING COUNT(*) > 1",
            &mut kv,
        )
        .unwrap();
        // M27 修复：HAVING 聚合列不在 SELECT 列表时仍应过滤
        // eng 组有 2 行（>1），应保留
        assert_eq!(result.rows.len(), 1);
    }
}
