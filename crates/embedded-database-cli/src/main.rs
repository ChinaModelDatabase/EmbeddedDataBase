// crates/embedded-database-cli/src/main.rs
//! embedded-database 交互式 SQL shell。

use embedded_database_core::Config;
use embedded_database_kv::KvDb;
use embedded_database_os::FileDevice;
use embedded_database_sql::{execute, Literal, QueryResult};
use std::io::{self, Write};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let db_path = if args.len() > 1 {
        args[1].clone()
    } else {
        "embedded-database.db".to_string()
    };

    println!("embedded-database CLI — 数据库: {db_path}");
    println!("输入 SQL 语句，输入 .exit 退出");

    let dev = match FileDevice::open(Path::new(&db_path), 64 << 20) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("无法打开数据库: {e:?}");
            std::process::exit(1);
        }
    };
    let mut dev = dev;
    let mut kv = match KvDb::open(&mut dev, Config::default()) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("无法初始化数据库: {e:?}");
            std::process::exit(1);
        }
    };

    loop {
        print!("embedded-database> ");
        io::stdout().flush().unwrap();

        let mut line = String::new();
        if io::stdin().read_line(&mut line).unwrap() == 0 {
            break; // EOF
        }
        let sql = line.trim();
        if sql.is_empty() {
            continue;
        }
        if sql == ".exit" || sql == ".quit" {
            break;
        }
        if sql == ".help" {
            println!("可用命令:");
            println!("  SELECT ... FROM ... [WHERE ...]");
            println!("  INSERT INTO ... VALUES (...)");
            println!("  CREATE TABLE ... (...)");
            println!("  DROP TABLE ...");
            println!("  .exit / .quit  退出");
            println!("  .help          帮助");
            continue;
        }

        match execute(sql, &mut kv) {
            Ok(result) => print_result(&result),
            Err(e) => eprintln!("错误: {e:?}"),
        }
    }

    let _ = kv.flush();
    println!("再见");
}

/// 格式化输出查询结果。
fn print_result(result: &QueryResult) {
    if result.columns.is_empty() && result.rows.is_empty() {
        println!("OK");
        return;
    }
    // 表头
    if !result.columns.is_empty() {
        let header: Vec<String> = result.columns.iter().map(|c| format!("{c:>10}")).collect();
        println!("{}", header.join(" | "));
        let sep: Vec<String> = result
            .columns
            .iter()
            .map(|_| "----------".to_string())
            .collect();
        println!("{}", sep.join("-+-"));
    }
    // 数据行
    for row in &result.rows {
        let cells: Vec<String> = row
            .iter()
            .map(|lit| format!("{:>10}", literal_str(lit)))
            .collect();
        println!("{}", cells.join(" | "));
    }
    println!("({} rows)", result.rows.len());
}

/// 字面量转显示字符串。
fn literal_str(lit: &Literal) -> String {
    match lit {
        Literal::Integer(v) => v.to_string(),
        Literal::Float(v) => v.to_string(),
        Literal::StringLit(s) => s.clone(),
        Literal::Null => "NULL".to_string(),
    }
}
