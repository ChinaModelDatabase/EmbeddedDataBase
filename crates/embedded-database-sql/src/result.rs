//! SQL 查询结果类型。

use crate::ast::Literal;
use crate::lexer::LexError;
use crate::parser::ParseError;
use alloc::string::String;
use alloc::vec::Vec;

/// SQL 运行时值（函数/聚合输入输出）。
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Text(String),
    Boolean(bool),
    Blob(Vec<u8>),
    Null,
}

/// SQL 错误。
#[derive(Debug, Clone, PartialEq)]
pub enum SqlError {
    Lex(LexError),
    Parse(ParseError),
    Db(String),
    InvalidArgument,
}

/// 查询结果。
#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    /// 列名。
    pub columns: Vec<String>,
    /// 数据行。
    pub rows: Vec<Vec<Literal>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn result_创建() {
        let r = QueryResult {
            columns: vec!["a".into(), "b".into()],
            rows: vec![vec![Literal::Integer(1), Literal::StringLit("x".into())]],
        };
        assert_eq!(r.columns.len(), 2);
        assert_eq!(r.rows.len(), 1);
    }

    #[test]
    fn result_空() {
        let r = QueryResult {
            columns: vec![],
            rows: vec![],
        };
        assert!(r.rows.is_empty());
    }

    #[test]
    fn 值序列化_布尔() {
        assert_eq!(Value::Boolean(true), Value::Boolean(true));
        assert_ne!(Value::Boolean(true), Value::Boolean(false));
        assert_ne!(Value::Boolean(true), Value::Int(1));
        assert_eq!(Value::Boolean(false).clone(), Value::Boolean(false));
    }

    #[test]
    fn 值序列化_blob() {
        let data = alloc::vec![0xde, 0xad, 0xbe, 0xef];
        assert_eq!(Value::Blob(data.clone()), Value::Blob(data.clone()));
        assert_ne!(
            Value::Blob(data.clone()),
            Value::Blob(alloc::vec![0, 1, 2, 3])
        );
        assert_ne!(Value::Blob(data.clone()), Value::Text("deadbeef".into()));
    }
}
