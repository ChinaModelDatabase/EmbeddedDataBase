//! SQL 类型扩展 trait。

use crate::result::SqlError;
use crate::result::Value;

/// SQL 类型 trait。
///
/// 扩展实现者必须满足：`parse`/`format` 永不 panic。
pub trait SqlType {
    /// 类型名（如 "date"、"uuid"）。
    fn type_name(&self) -> &str;

    /// 从字符串解析值。
    fn parse(&self, s: &str) -> Result<Value, SqlError>;

    /// 格式化值为字符串。
    fn format(&self, v: &Value) -> alloc::string::String;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::string::ToString;

    struct DateType;

    impl SqlType for DateType {
        fn type_name(&self) -> &str {
            "date"
        }
        fn parse(&self, s: &str) -> Result<Value, SqlError> {
            let parts: alloc::vec::Vec<&str> = s.split('-').collect();
            if parts.len() == 3
                && parts[0].len() == 4
                && parts[1].len() == 2
                && parts[2].len() == 2
                && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
            {
                Ok(Value::Text(s.to_string()))
            } else {
                Err(SqlError::InvalidArgument)
            }
        }
        fn format(&self, v: &Value) -> String {
            match v {
                Value::Text(s) => s.clone(),
                _ => String::new(),
            }
        }
    }

    #[test]
    fn 自定义类型_date() {
        let date = DateType;
        assert_eq!(date.type_name(), "date");
        let got = date.parse("2026-10-05").unwrap();
        assert_eq!(got, Value::Text("2026-10-05".to_string()));
        assert!(date.parse("2026-1-5").is_err());
        assert!(date.parse("not-a-date").is_err());
        assert_eq!(date.format(&got), "2026-10-05".to_string());
        assert_eq!(date.format(&Value::Int(1)), String::new());
    }
}
