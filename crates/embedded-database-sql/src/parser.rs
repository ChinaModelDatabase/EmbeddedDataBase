//! SQL 递归下降语法分析器。

use crate::ast::*;
use crate::token::{Keyword, Token, TokenType};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// 语法错误。
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub pos: usize,
    pub msg: String,
}

fn err_at(tokens: &[Token], i: usize, msg: &str) -> ParseError {
    ParseError {
        pos: tokens.get(i).map_or(0, |t| t.pos),
        msg: msg.into(),
    }
}

/// 解析 SQL 语句。
pub fn parse(tokens: &[Token]) -> Result<Statement, ParseError> {
    if tokens.is_empty() {
        return Err(ParseError {
            pos: 0,
            msg: "empty input".into(),
        });
    }
    match &tokens[0].ty {
        TokenType::Keyword(Keyword::Select) => parse_select(tokens),
        TokenType::Keyword(Keyword::Insert) => parse_insert(tokens),
        TokenType::Keyword(Keyword::Create) => parse_create_table(tokens),
        TokenType::Keyword(Keyword::Drop) => parse_drop_table(tokens),
        _ => Err(ParseError {
            pos: tokens[0].pos,
            msg: "unexpected token".into(),
        }),
    }
}

/// 解析 SELECT 语句。
fn parse_select(tokens: &[Token]) -> Result<Statement, ParseError> {
    let mut i = 1; // 跳过 SELECT
    let mut columns = Vec::new();

    // 解析列列表
    loop {
        if i >= tokens.len() {
            return Err(err_at(tokens, i, "unexpected end"));
        }
        match &tokens[i].ty {
            TokenType::Punct('*') => {
                columns.push(SelectItem::Wildcard);
                i += 1;
            }
            TokenType::Identifier(name) => {
                columns.push(SelectItem::Column(name.clone()));
                i += 1;
            }
            TokenType::Keyword(kw) => {
                let agg = match kw {
                    Keyword::Count => Some(AggregateFn::Count),
                    Keyword::Sum => Some(AggregateFn::Sum),
                    Keyword::Avg => Some(AggregateFn::Avg),
                    Keyword::Min => Some(AggregateFn::Min),
                    Keyword::Max => Some(AggregateFn::Max),
                    _ => None,
                };
                let Some(f) = agg else {
                    return Err(err_at(tokens, i, "expected column"));
                };
                i += 1;
                if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Punct('(')) {
                    return Err(err_at(tokens, i, "expected ( after aggregate"));
                }
                i += 1;
                if i >= tokens.len() {
                    return Err(err_at(tokens, i, "unexpected end"));
                }
                match &tokens[i].ty {
                    TokenType::Punct('*') => i += 1,
                    TokenType::Identifier(_) => i += 1,
                    _ => return Err(err_at(tokens, i, "expected * or column in aggregate")),
                }
                if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Punct(')')) {
                    return Err(err_at(tokens, i, "expected )"));
                }
                i += 1;
                columns.push(SelectItem::Aggregate(f));
            }
            _ => return Err(err_at(tokens, i, "expected column")),
        }
        if i < tokens.len() && matches!(tokens[i].ty, TokenType::Punct(',')) {
            i += 1;
            continue;
        }
        break;
    }

    // FROM
    let mut from = None;
    if i < tokens.len() {
        if let TokenType::Keyword(Keyword::From) = tokens[i].ty {
            i += 1;
            if i < tokens.len() {
                if let TokenType::Identifier(name) = &tokens[i].ty {
                    from = Some(name.clone());
                    i += 1;
                }
            }
        }
    }

    // JOIN（M23：`FROM t1 JOIN t2 ON t1.col = t2.col`，ON 仅支持 `=`）
    let mut joins = Vec::new();
    while i < tokens.len() {
        if let TokenType::Keyword(Keyword::Join) = tokens[i].ty {
            i += 1;
            // 表名
            if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Identifier(_)) {
                return Err(err_at(tokens, i, "expected table name"));
            }
            let table = match &tokens[i].ty {
                TokenType::Identifier(n) => n.clone(),
                _ => return Err(err_at(tokens, i, "expected table name")),
            };
            i += 1;
            // ON 条件
            if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Keyword(Keyword::On)) {
                return Err(err_at(tokens, i, "expected ON"));
            }
            i += 1;
            // left = right
            if i + 2 >= tokens.len() {
                return Err(err_at(tokens, i, "unexpected end"));
            }
            let on_left = match &tokens[i].ty {
                TokenType::Identifier(n) => n.clone(),
                _ => return Err(err_at(tokens, i, "expected column")),
            };
            i += 1;
            if !matches!(tokens[i].ty, TokenType::Punct('=')) {
                return Err(err_at(tokens, i, "expected ="));
            }
            i += 1;
            let on_right = match &tokens[i].ty {
                TokenType::Identifier(n) => n.clone(),
                _ => return Err(err_at(tokens, i, "expected column")),
            };
            i += 1;
            joins.push(JoinClause {
                table,
                alias: None,
                on_left,
                on_right,
            });
        } else {
            break;
        }
    }

    // WHERE: 仅支持 column op literal（= / < / > / != / <= / >=）
    let mut where_clause = None;
    if i < tokens.len() {
        if let TokenType::Keyword(Keyword::Where) = tokens[i].ty {
            i += 1;
            if i + 2 < tokens.len() {
                if let (TokenType::Identifier(col), TokenType::Punct(op)) =
                    (&tokens[i].ty, &tokens[i + 1].ty)
                {
                    let col_name = col.clone();
                    // 默认字面量位置：单字符运算符之后
                    let mut lit_idx = i + 2;
                    // 二字符运算符：检查 tokens[i+2] 是否为 '='
                    let two_char = matches!(op, '<' | '>' | '!')
                        && i + 2 < tokens.len()
                        && matches!(tokens[i + 2].ty, TokenType::Punct('='));
                    if two_char {
                        lit_idx = i + 3;
                    }
                    let bin_op = match op {
                        '=' => BinOp::Eq,
                        '<' => {
                            if two_char {
                                BinOp::Le
                            } else {
                                BinOp::Lt
                            }
                        }
                        '>' => {
                            if two_char {
                                BinOp::Ge
                            } else {
                                BinOp::Gt
                            }
                        }
                        '!' => {
                            if two_char {
                                BinOp::Ne
                            } else {
                                return Err(ParseError {
                                    pos: tokens[i].pos,
                                    msg: "expected = after !".into(),
                                });
                            }
                        }
                        _ => {
                            return Err(ParseError {
                                pos: tokens[i].pos,
                                msg: "unsupported operator".into(),
                            })
                        }
                    };
                    if lit_idx >= tokens.len() {
                        return Err(ParseError {
                            pos: tokens[i].pos,
                            msg: "expected literal".into(),
                        });
                    }
                    let lit = match &tokens[lit_idx].ty {
                        TokenType::Integer(v) => Expr::Literal(Literal::Integer(*v)),
                        TokenType::Float(v) => Expr::Literal(Literal::Float(*v)),
                        TokenType::StringLit(s) => Expr::Literal(Literal::StringLit(s.clone())),
                        TokenType::Keyword(Keyword::True) => Expr::Literal(Literal::Integer(1)),
                        TokenType::Keyword(Keyword::False) => Expr::Literal(Literal::Integer(0)),
                        _ => {
                            return Err(ParseError {
                                pos: tokens[lit_idx].pos,
                                msg: "expected literal".into(),
                            })
                        }
                    };
                    where_clause = Some(Expr::BinaryOp(
                        Box::new(Expr::Column(col_name)),
                        bin_op,
                        Box::new(lit),
                    ));
                }
            }
        }
    }

    // ORDER BY（多列：col [ASC|DESC] [, col [ASC|DESC]]*）
    let mut order_by = Vec::new();
    if i < tokens.len() {
        if let TokenType::Keyword(Keyword::Order) = tokens[i].ty {
            i += 1;
            if i < tokens.len() && matches!(tokens[i].ty, TokenType::Keyword(Keyword::By)) {
                i += 1;
                loop {
                    let Some(token) = tokens.get(i) else {
                        break;
                    };
                    let TokenType::Identifier(col) = &token.ty else {
                        break;
                    };
                    let col = col.clone();
                    i += 1;
                    let mut asc = true;
                    if i < tokens.len() {
                        if let TokenType::Keyword(Keyword::Desc) = tokens[i].ty {
                            asc = false;
                            i += 1;
                        } else if let TokenType::Keyword(Keyword::Asc) = tokens[i].ty {
                            i += 1;
                        }
                    }
                    order_by.push(OrderByItem {
                        expr: Expr::Column(col),
                        asc,
                    });
                    if i < tokens.len() && tokens[i].ty == TokenType::Punct(',') {
                        i += 1;
                        continue;
                    }
                    break;
                }
            }
        }
    }

    // LIMIT
    let mut limit = None;
    if i < tokens.len() {
        if let TokenType::Keyword(Keyword::Limit) = tokens[i].ty {
            i += 1;
            if i < tokens.len() {
                if let TokenType::Integer(v) = tokens[i].ty {
                    limit = Some(v);
                }
            }
        }
    }

    // GROUP BY
    let mut group_by = Vec::new();
    if i < tokens.len() {
        if let TokenType::Keyword(Keyword::Group) = tokens[i].ty {
            i += 1;
            if i < tokens.len() && matches!(tokens[i].ty, TokenType::Keyword(Keyword::By)) {
                i += 1;
                // 解析分组列
                loop {
                    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Identifier(_)) {
                        break;
                    }
                    if let TokenType::Identifier(col) = &tokens[i].ty {
                        group_by.push(col.clone());
                    }
                    i += 1;
                    if i < tokens.len() {
                        if let TokenType::Punct(',') = tokens[i].ty {
                            i += 1;
                            continue;
                        }
                    }
                    break;
                }
            }
        }
    }

    // HAVING
    let mut having = None;
    if i < tokens.len() {
        if let TokenType::Keyword(Keyword::Having) = tokens[i].ty {
            i += 1;
            // 解析简单条件: aggregate op literal
            if i + 2 < tokens.len() {
                if let TokenType::Keyword(kw) = &tokens[i].ty {
                    let agg = match kw {
                        Keyword::Count => AggregateFn::Count,
                        Keyword::Sum => AggregateFn::Sum,
                        Keyword::Avg => AggregateFn::Avg,
                        Keyword::Min => AggregateFn::Min,
                        Keyword::Max => AggregateFn::Max,
                        _ => {
                            return Err(ParseError {
                                pos: tokens[i].pos,
                                msg: "expected aggregate".into(),
                            })
                        }
                    };
                    i += 1;
                    // 跳过括号（如 COUNT(*)）
                    if i < tokens.len() && matches!(tokens[i].ty, TokenType::Punct('(')) {
                        i += 1;
                        if i < tokens.len() && matches!(tokens[i].ty, TokenType::Punct('*')) {
                            i += 1;
                        }
                        if i < tokens.len() && matches!(tokens[i].ty, TokenType::Punct(')')) {
                            i += 1;
                        }
                    }
                    // 运算符
                    if i >= tokens.len() {
                        return Err(ParseError {
                            pos: 0,
                            msg: "unexpected end".into(),
                        });
                    }
                    let bin_op = match &tokens[i].ty {
                        TokenType::Punct('=') => BinOp::Eq,
                        TokenType::Punct('>') => BinOp::Gt,
                        TokenType::Punct('<') => BinOp::Lt,
                        _ => {
                            return Err(ParseError {
                                pos: tokens[i].pos,
                                msg: "expected operator".into(),
                            })
                        }
                    };
                    i += 1;
                    // 字面量
                    if i >= tokens.len() {
                        return Err(ParseError {
                            pos: 0,
                            msg: "unexpected end".into(),
                        });
                    }
                    let lit = match &tokens[i].ty {
                        TokenType::Integer(v) => Expr::Literal(Literal::Integer(*v)),
                        TokenType::Float(v) => Expr::Literal(Literal::Float(*v)),
                        _ => {
                            return Err(ParseError {
                                pos: tokens[i].pos,
                                msg: "expected literal".into(),
                            })
                        }
                    };
                    having = Some(Expr::BinaryOp(
                        Box::new(Expr::Aggregate(agg)),
                        bin_op,
                        Box::new(lit),
                    ));
                }
            }
        }
    }

    Ok(Statement::Select(SelectStmt {
        columns,
        from,
        joins,
        where_clause,
        order_by,
        limit,
        group_by,
        having,
    }))
}

/// 解析 INSERT 语句。
fn parse_insert(tokens: &[Token]) -> Result<Statement, ParseError> {
    let mut i = 1; // 跳过 INSERT
                   // INTO
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Keyword(Keyword::Into)) {
        return Err(err_at(tokens, i, "expected INTO"));
    }
    i += 1;
    // 表名
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Identifier(_)) {
        return Err(err_at(tokens, i, "expected table name"));
    }
    let table = match &tokens[i].ty {
        TokenType::Identifier(name) => name.clone(),
        _ => return Err(err_at(tokens, i, "expected table name")),
    };
    i += 1;

    // 列列表
    let mut columns = Vec::new();
    if i < tokens.len() && matches!(tokens[i].ty, TokenType::Punct('(')) {
        i += 1;
        loop {
            if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Identifier(_)) {
                return Err(err_at(tokens, i, "expected column"));
            }
            if let TokenType::Identifier(name) = &tokens[i].ty {
                columns.push(name.clone());
            }
            i += 1;
            if i < tokens.len() && matches!(tokens[i].ty, TokenType::Punct(',')) {
                i += 1;
                continue;
            }
            break;
        }
        if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Punct(')')) {
            return Err(err_at(tokens, i, "expected )"));
        }
        i += 1;
    }

    // VALUES
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Keyword(Keyword::Values)) {
        return Err(err_at(tokens, i, "expected VALUES"));
    }
    i += 1;

    // 值列表
    let mut values = Vec::new();
    loop {
        if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Punct('(')) {
            return Err(err_at(tokens, i, "expected ("));
        }
        i += 1;
        let mut row = Vec::new();
        loop {
            if i >= tokens.len() {
                return Err(err_at(tokens, i, "unexpected end"));
            }
            match &tokens[i].ty {
                TokenType::Integer(v) => {
                    row.push(Expr::Literal(Literal::Integer(*v)));
                    i += 1;
                }
                TokenType::Float(v) => {
                    row.push(Expr::Literal(Literal::Float(*v)));
                    i += 1;
                }
                TokenType::StringLit(s) => {
                    row.push(Expr::Literal(Literal::StringLit(s.clone())));
                    i += 1;
                }
                TokenType::Keyword(Keyword::Null) => {
                    row.push(Expr::Literal(Literal::Null));
                    i += 1;
                }
                TokenType::Keyword(Keyword::True) => {
                    row.push(Expr::Literal(Literal::Integer(1)));
                    i += 1;
                }
                TokenType::Keyword(Keyword::False) => {
                    row.push(Expr::Literal(Literal::Integer(0)));
                    i += 1;
                }
                _ => return Err(err_at(tokens, i, "expected literal")),
            }
            if i < tokens.len() && matches!(tokens[i].ty, TokenType::Punct(',')) {
                i += 1;
                continue;
            }
            break;
        }
        if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Punct(')')) {
            return Err(err_at(tokens, i, "expected )"));
        }
        i += 1;
        values.push(row);
        if i < tokens.len() && matches!(tokens[i].ty, TokenType::Punct(',')) {
            i += 1;
            continue;
        }
        break;
    }

    Ok(Statement::Insert(InsertStmt {
        table,
        columns,
        values,
    }))
}

/// 解析 CREATE TABLE 语句。
fn parse_create_table(tokens: &[Token]) -> Result<Statement, ParseError> {
    let mut i = 1; // 跳过 CREATE
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Keyword(Keyword::Table)) {
        return Err(err_at(tokens, i, "expected TABLE"));
    }
    i += 1;
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Identifier(_)) {
        return Err(err_at(tokens, i, "expected table name"));
    }
    let table = match &tokens[i].ty {
        TokenType::Identifier(name) => name.clone(),
        _ => return Err(err_at(tokens, i, "expected table name")),
    };
    i += 1;

    // 列定义
    let mut columns = Vec::new();
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Punct('(')) {
        return Err(err_at(tokens, i, "expected ("));
    }
    i += 1;
    loop {
        if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Identifier(_)) {
            return Err(err_at(tokens, i, "expected column name"));
        }
        let name = match &tokens[i].ty {
            TokenType::Identifier(n) => n.clone(),
            _ => return Err(err_at(tokens, i, "expected column name")),
        };
        i += 1;

        // 类型
        if i >= tokens.len() {
            return Err(err_at(tokens, i, "unexpected end"));
        }
        let col_type = match &tokens[i].ty {
            TokenType::Identifier(t) => t.clone(),
            TokenType::Keyword(k) => format!("{k:?}"),
            _ => return Err(err_at(tokens, i, "expected type")),
        };
        i += 1;

        let mut primary_key = false;
        let mut not_null = false;

        // 约束
        while i < tokens.len() {
            match &tokens[i].ty {
                TokenType::Keyword(Keyword::Primary) => {
                    i += 1;
                    if i < tokens.len() && matches!(tokens[i].ty, TokenType::Keyword(Keyword::Key))
                    {
                        primary_key = true;
                        i += 1;
                    }
                }
                TokenType::Keyword(Keyword::Not) => {
                    i += 1;
                    if i < tokens.len() && matches!(tokens[i].ty, TokenType::Keyword(Keyword::Null))
                    {
                        not_null = true;
                        i += 1;
                    }
                }
                _ => break,
            }
        }

        columns.push(ColumnDef {
            name,
            col_type,
            primary_key,
            not_null,
        });

        if i < tokens.len() && matches!(tokens[i].ty, TokenType::Punct(',')) {
            i += 1;
            continue;
        }
        break;
    }
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Punct(')')) {
        return Err(err_at(tokens, i, "expected )"));
    }

    Ok(Statement::CreateTable(CreateTableStmt { table, columns }))
}

/// 解析 DROP TABLE 语句。
fn parse_drop_table(tokens: &[Token]) -> Result<Statement, ParseError> {
    let mut i = 1; // 跳过 DROP
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Keyword(Keyword::Table)) {
        return Err(err_at(tokens, i, "expected TABLE"));
    }
    i += 1;
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Identifier(_)) {
        return Err(err_at(tokens, i, "expected table name"));
    }
    let table = match &tokens[i].ty {
        TokenType::Identifier(name) => name.clone(),
        _ => return Err(err_at(tokens, i, "expected table name")),
    };
    Ok(Statement::DropTable(DropTableStmt { table }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn 解析_select_基本() {
        let tokens = tokenize("SELECT * FROM users").unwrap();
        let stmt = parse(&tokens).unwrap();
        assert!(matches!(stmt, Statement::Select(_)));
        if let Statement::Select(s) = stmt {
            assert_eq!(s.columns, vec![SelectItem::Wildcard]);
            assert_eq!(s.from, Some("users".into()));
            assert!(s.where_clause.is_none());
        }
    }

    #[test]
    fn 解析_select_列名() {
        let tokens = tokenize("SELECT name, age FROM users").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert_eq!(
                s.columns,
                vec![
                    SelectItem::Column("name".into()),
                    SelectItem::Column("age".into())
                ]
            );
        }
    }

    #[test]
    fn 解析_insert_基本() {
        let tokens = tokenize("INSERT INTO users (name, age) VALUES ('alice', 30)").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Insert(s) = stmt {
            assert_eq!(s.table, "users");
            assert_eq!(s.columns, vec!["name".to_string(), "age".to_string()]);
            assert_eq!(s.values.len(), 1);
            assert_eq!(s.values[0].len(), 2);
        }
    }

    #[test]
    fn 解析_create_table() {
        let tokens =
            tokenize("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::CreateTable(s) = stmt {
            assert_eq!(s.table, "users");
            assert_eq!(s.columns.len(), 2);
            assert!(s.columns[0].primary_key);
            assert!(s.columns[1].not_null);
        }
    }

    #[test]
    fn 解析_drop_table() {
        let tokens = tokenize("DROP TABLE users").unwrap();
        let stmt = parse(&tokens).unwrap();
        assert!(matches!(stmt, Statement::DropTable(DropTableStmt { table }) if table == "users"));
    }

    #[test]
    fn 解析_空输入报错() {
        let tokens = tokenize("").unwrap();
        assert!(parse(&tokens).is_err());
    }

    #[test]
    fn 解析_未知语句报错() {
        let tokens = tokenize("FOO BAR").unwrap();
        assert!(parse(&tokens).is_err());
    }

    #[test]
    fn 解析_select_order_by() {
        let tokens = tokenize("SELECT name FROM users ORDER BY name ASC").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert_eq!(s.order_by.len(), 1);
            assert!(s.order_by[0].asc);
        }
    }

    #[test]
    fn 解析_select_order_by_desc() {
        let tokens = tokenize("SELECT name FROM users ORDER BY age DESC").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert_eq!(s.order_by.len(), 1);
            assert!(!s.order_by[0].asc);
        }
    }

    #[test]
    fn 解析_select_limit() {
        let tokens = tokenize("SELECT name FROM users LIMIT 10").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert_eq!(s.limit, Some(10));
        }
    }

    #[test]
    fn 解析_select_order_by_and_limit() {
        let tokens = tokenize("SELECT name FROM users ORDER BY name ASC LIMIT 5").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert_eq!(s.order_by.len(), 1);
            assert_eq!(s.limit, Some(5));
        }
    }

    #[test]
    fn 解析_where_ne() {
        let tokens = tokenize("SELECT name FROM users WHERE age != 30").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert!(s.where_clause.is_some());
        }
    }

    #[test]
    fn 解析_where_le() {
        let tokens = tokenize("SELECT name FROM users WHERE age <= 30").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert!(s.where_clause.is_some());
        }
    }

    #[test]
    fn 解析_where_ge() {
        let tokens = tokenize("SELECT name FROM users WHERE age >= 30").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert!(s.where_clause.is_some());
        }
    }

    #[test]
    fn 解析_select_join() {
        let tokens =
            tokenize("SELECT users.name FROM users JOIN orders ON users.id = orders.user_id")
                .unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert_eq!(s.joins.len(), 1);
            assert_eq!(s.joins[0].table, "orders");
            assert_eq!(s.joins[0].on_left, "users.id");
            assert_eq!(s.joins[0].on_right, "orders.user_id");
        }
    }

    #[test]
    fn 解析_select_无join() {
        let tokens = tokenize("SELECT name FROM users").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert!(s.joins.is_empty());
        }
    }

    #[test]
    fn 解析_select_count() {
        let tokens = tokenize("SELECT COUNT(*) FROM users").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert_eq!(s.columns, vec![SelectItem::Aggregate(AggregateFn::Count)]);
        }
    }

    #[test]
    fn 解析_select_sum() {
        let tokens = tokenize("SELECT SUM(age) FROM users").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert_eq!(s.columns, vec![SelectItem::Aggregate(AggregateFn::Sum)]);
        }
    }

    #[test]
    fn 解析_select_avg() {
        let tokens = tokenize("SELECT AVG(age) FROM users").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert_eq!(s.columns, vec![SelectItem::Aggregate(AggregateFn::Avg)]);
        }
    }

    #[test]
    fn 解析_select_min_max() {
        let tokens = tokenize("SELECT MIN(age), MAX(age) FROM users").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert_eq!(
                s.columns,
                vec![
                    SelectItem::Aggregate(AggregateFn::Min),
                    SelectItem::Aggregate(AggregateFn::Max),
                ]
            );
        }
    }

    #[test]
    fn 解析_select_group_by() {
        let tokens = tokenize("SELECT name, COUNT(*) FROM users GROUP BY name").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert_eq!(s.group_by, vec!["name".to_string()]);
        }
    }

    #[test]
    fn 解析_select_无group_by() {
        let tokens = tokenize("SELECT name FROM users").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert!(s.group_by.is_empty());
        }
    }

    #[test]
    fn 解析_select_having() {
        let tokens =
            tokenize("SELECT dept, COUNT(*) FROM users GROUP BY dept HAVING COUNT(*) > 1").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert!(s.having.is_some());
        }
    }

    #[test]
    fn 解析_select_无_having() {
        let tokens = tokenize("SELECT name FROM users").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert!(s.having.is_none());
        }
    }

    #[test]
    fn 布尔字面量_true_false() {
        let tokens = tokenize("SELECT name FROM users WHERE active = TRUE").unwrap();
        assert_eq!(tokens[7].ty, TokenType::Keyword(Keyword::True));
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert!(s.where_clause.is_some());
        }

        let tokens = tokenize("INSERT INTO t VALUES (TRUE, FALSE)").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Insert(s) = stmt {
            assert_eq!(s.values[0].len(), 2);
            assert_eq!(s.values[0][0], Expr::Literal(Literal::Integer(1)));
            assert_eq!(s.values[0][1], Expr::Literal(Literal::Integer(0)));
        }
    }
}
