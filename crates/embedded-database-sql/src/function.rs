//! SQL 函数扩展 trait。
//!
//! 设计原则：编译期静态分发、trait 约束、无动态注册。
//! 扩展实现者必须满足：永不 panic、时延有界。

use crate::result::{SqlError, Value};
use alloc::string::String;
use alloc::vec::Vec;

/// 标量函数 trait。
///
/// 实现者必须满足：
/// - `eval` 永不 panic，错误通过 `Err` 返回
/// - `eval` 时延有界（输入规模固定时）
pub trait ScalarFunction {
    /// 函数名（小写，如 "upper"）。
    fn name(&self) -> &str;

    /// 参数个数。`None` 表示变参。
    fn arity(&self) -> Option<usize>;

    /// 求值。`args.len()` 必须与 `arity()` 一致（变参时任意）。
    fn eval(&self, args: &[Value]) -> Result<Value, SqlError>;
}

/// 聚合函数 trait。
///
/// 实现者必须满足：
/// - `step`/`finalize` 永不 panic
/// - 时延有界
pub trait AggregateFunction {
    /// 函数名（小写，如 "sum"）。
    fn name(&self) -> &str;

    /// 初始化聚合状态。
    fn init(&self) -> AggregateState;

    /// 累加一个值到状态。
    fn step(&self, state: &mut AggregateState, arg: Value) -> Result<(), SqlError>;

    /// 从状态计算最终值。
    fn finalize(&self, state: &AggregateState) -> Value;
}

/// 聚合状态。用 enum 避免动态分配。
#[derive(Debug, Clone)]
pub enum AggregateState {
    /// 无状态（如 COUNT）。
    None,
    /// 数值累加（如 SUM/AVG）。
    Sum { count: u64, total: f64 },
    /// 最小值（如 MIN）。
    Min(f64),
    /// 最大值（如 MAX）。
    Max(f64),
    /// 自定义状态（扩展用）。
    Custom(u64),
}

/// 内置标量函数注册表。
pub struct BuiltinScalar;

impl BuiltinScalar {
    /// 按名称获取标量函数。
    pub fn get(name: &str) -> Option<&'static dyn ScalarFunction> {
        match name {
            "upper" => Some(&UPPER),
            "lower" => Some(&LOWER),
            "length" => Some(&LENGTH),
            "substr" => Some(&SUBSTR),
            "abs" => Some(&ABS),
            "round" => Some(&ROUND),
            "coalesce" => Some(&COALESCE),
            _ => None,
        }
    }
}

impl ScalarFunction for BuiltinScalar {
    fn name(&self) -> &str {
        "upper"
    }
    fn arity(&self) -> Option<usize> {
        Some(1)
    }
    fn eval(&self, args: &[Value]) -> Result<Value, SqlError> {
        UpperFn.eval(args)
    }
}

struct UpperFn;
static UPPER: UpperFn = UpperFn;

struct LowerFn;
struct LengthFn;
struct SubstrFn;
struct AbsFn;
struct RoundFn;
struct CoalesceFn;

static LOWER: LowerFn = LowerFn;
static LENGTH: LengthFn = LengthFn;
static SUBSTR: SubstrFn = SubstrFn;
static ABS: AbsFn = AbsFn;
static ROUND: RoundFn = RoundFn;
static COALESCE: CoalesceFn = CoalesceFn;

impl ScalarFunction for UpperFn {
    fn name(&self) -> &str {
        "upper"
    }
    fn arity(&self) -> Option<usize> {
        Some(1)
    }
    fn eval(&self, args: &[Value]) -> Result<Value, SqlError> {
        match args.first() {
            Some(Value::Text(s)) => Ok(Value::Text(s.to_uppercase())),
            _ => Err(SqlError::InvalidArgument),
        }
    }
}

impl ScalarFunction for LowerFn {
    fn name(&self) -> &str {
        "lower"
    }
    fn arity(&self) -> Option<usize> {
        Some(1)
    }
    fn eval(&self, args: &[Value]) -> Result<Value, SqlError> {
        match args.first() {
            Some(Value::Text(s)) => Ok(Value::Text(s.to_lowercase())),
            _ => Err(SqlError::InvalidArgument),
        }
    }
}

impl ScalarFunction for LengthFn {
    fn name(&self) -> &str {
        "length"
    }
    fn arity(&self) -> Option<usize> {
        Some(1)
    }
    fn eval(&self, args: &[Value]) -> Result<Value, SqlError> {
        match args.first() {
            Some(Value::Text(s)) => Ok(Value::Int(s.chars().count() as i64)),
            Some(Value::Blob(b)) => Ok(Value::Int(b.len() as i64)),
            _ => Err(SqlError::InvalidArgument),
        }
    }
}

impl ScalarFunction for SubstrFn {
    fn name(&self) -> &str {
        "substr"
    }
    fn arity(&self) -> Option<usize> {
        Some(3)
    }
    fn eval(&self, args: &[Value]) -> Result<Value, SqlError> {
        let (s, start, len) = match (args.first(), args.get(1), args.get(2)) {
            (Some(Value::Text(s)), Some(Value::Int(start)), Some(Value::Int(len))) => {
                (s.as_str(), *start, *len)
            }
            _ => return Err(SqlError::InvalidArgument),
        };
        if len <= 0 {
            return Ok(Value::Text(String::new()));
        }
        let chars: Vec<char> = s.chars().collect();
        let n = chars.len() as i64;
        let start_idx = match start.cmp(&0) {
            core::cmp::Ordering::Greater => start - 1,
            core::cmp::Ordering::Less => n + start,
            core::cmp::Ordering::Equal => 0,
        };
        if start_idx < 0 || start_idx >= n {
            return Ok(Value::Text(String::new()));
        }
        let begin = start_idx as usize;
        let end = begin.saturating_add(len as usize).min(chars.len());
        Ok(Value::Text(chars[begin..end].iter().collect()))
    }
}

impl ScalarFunction for AbsFn {
    fn name(&self) -> &str {
        "abs"
    }
    fn arity(&self) -> Option<usize> {
        Some(1)
    }
    fn eval(&self, args: &[Value]) -> Result<Value, SqlError> {
        match args.first() {
            Some(Value::Int(v)) => match v.checked_abs() {
                Some(a) => Ok(Value::Int(a)),
                None => Ok(Value::Float(f64_abs(i64::MIN as f64))),
            },
            Some(Value::Float(v)) => Ok(Value::Float(f64_abs(*v))),
            _ => Err(SqlError::InvalidArgument),
        }
    }
}

impl ScalarFunction for RoundFn {
    fn name(&self) -> &str {
        "round"
    }
    fn arity(&self) -> Option<usize> {
        Some(2)
    }
    fn eval(&self, args: &[Value]) -> Result<Value, SqlError> {
        let (n, d) = match (args.first(), args.get(1)) {
            (Some(Value::Int(v)), Some(Value::Int(d))) => (*v as f64, *d),
            (Some(Value::Float(v)), Some(Value::Int(d))) => (*v, *d),
            _ => return Err(SqlError::InvalidArgument),
        };
        let d = d.clamp(-15, 15);
        let scaled = if d >= 0 {
            n * pow10(d as i32)
        } else {
            n / pow10(-d as i32)
        };
        let rounded = f64_round(scaled);
        if d >= 0 {
            Ok(Value::Float(rounded / pow10(d as i32)))
        } else {
            Ok(Value::Float(rounded * pow10(-d as i32)))
        }
    }
}

impl ScalarFunction for CoalesceFn {
    fn name(&self) -> &str {
        "coalesce"
    }
    fn arity(&self) -> Option<usize> {
        None
    }
    fn eval(&self, args: &[Value]) -> Result<Value, SqlError> {
        if args.is_empty() {
            return Err(SqlError::InvalidArgument);
        }
        for arg in args {
            if !matches!(arg, Value::Null) {
                return Ok(arg.clone());
            }
        }
        Ok(Value::Null)
    }
}

/// no_std 兼容：f64 绝对值（core 无 `f64::abs`）。
fn f64_abs(v: f64) -> f64 {
    if v < 0.0 {
        -v
    } else {
        v
    }
}

/// no_std 兼容：f64 向下取整（core 无 `f64::floor`）。
fn f64_floor(v: f64) -> f64 {
    if v >= 0.0 {
        v as i64 as f64
    } else {
        let t = v as i64 as f64;
        if t == v {
            t
        } else {
            t - 1.0
        }
    }
}

/// no_std 兼容：f64 四舍五入（half away from zero，core 无 `f64::round`）。
fn f64_round(v: f64) -> f64 {
    if v >= 0.0 {
        f64_floor(v + 0.5)
    } else {
        -f64_floor(-v + 0.5)
    }
}

/// no_std 兼容：10^d（d ∈ [0, 15]，core 无 `f64::powi`）。
fn pow10(d: i32) -> f64 {
    let mut f = 1.0f64;
    let mut i = 0;
    while i < d {
        f *= 10.0;
        i += 1;
    }
    f
}

/// 内置聚合函数注册表。
pub struct BuiltinAggregates;

impl BuiltinAggregates {
    /// 按名称获取聚合函数。
    pub fn get(name: &str) -> Option<&'static dyn AggregateFunction> {
        match name {
            "count" => Some(&CountFn),
            "sum" => Some(&SumFn),
            "avg" => Some(&AvgFn),
            "min" => Some(&MinFn),
            "max" => Some(&MaxFn),
            _ => None,
        }
    }
}

struct CountFn;
struct SumFn;
struct AvgFn;
struct MinFn;
struct MaxFn;

impl AggregateFunction for CountFn {
    fn name(&self) -> &str {
        "count"
    }
    fn init(&self) -> AggregateState {
        AggregateState::Sum {
            count: 0,
            total: 0.0,
        }
    }
    fn step(&self, state: &mut AggregateState, _arg: Value) -> Result<(), SqlError> {
        if let AggregateState::Sum { count, .. } = state {
            *count += 1;
        }
        Ok(())
    }
    fn finalize(&self, state: &AggregateState) -> Value {
        match state {
            AggregateState::Sum { count, .. } => Value::Int(*count as i64),
            _ => Value::Int(0),
        }
    }
}

impl AggregateFunction for SumFn {
    fn name(&self) -> &str {
        "sum"
    }
    fn init(&self) -> AggregateState {
        AggregateState::Sum {
            count: 0,
            total: 0.0,
        }
    }
    fn step(&self, state: &mut AggregateState, arg: Value) -> Result<(), SqlError> {
        if let (AggregateState::Sum { count, total }, Value::Int(v)) = (state, arg) {
            *count += 1;
            *total += v as f64;
        }
        Ok(())
    }
    fn finalize(&self, state: &AggregateState) -> Value {
        match state {
            AggregateState::Sum { total, .. } => Value::Int(*total as i64),
            _ => Value::Int(0),
        }
    }
}

impl AggregateFunction for AvgFn {
    fn name(&self) -> &str {
        "avg"
    }
    fn init(&self) -> AggregateState {
        AggregateState::Sum {
            count: 0,
            total: 0.0,
        }
    }
    fn step(&self, state: &mut AggregateState, arg: Value) -> Result<(), SqlError> {
        if let (AggregateState::Sum { count, total }, Value::Int(v)) = (state, arg) {
            *count += 1;
            *total += v as f64;
        }
        Ok(())
    }
    fn finalize(&self, state: &AggregateState) -> Value {
        match state {
            AggregateState::Sum { count, total } if *count > 0 => {
                Value::Float(*total / *count as f64)
            }
            _ => Value::Int(0),
        }
    }
}

impl AggregateFunction for MinFn {
    fn name(&self) -> &str {
        "min"
    }
    fn init(&self) -> AggregateState {
        AggregateState::Min(f64::MAX)
    }
    fn step(&self, state: &mut AggregateState, arg: Value) -> Result<(), SqlError> {
        if let (AggregateState::Min(m), Value::Int(v)) = (state, arg) {
            if (v as f64) < *m {
                *m = v as f64;
            }
        }
        Ok(())
    }
    fn finalize(&self, state: &AggregateState) -> Value {
        match state {
            AggregateState::Min(m) if *m < f64::MAX => Value::Int(*m as i64),
            _ => Value::Null,
        }
    }
}

impl AggregateFunction for MaxFn {
    fn name(&self) -> &str {
        "max"
    }
    fn init(&self) -> AggregateState {
        AggregateState::Max(f64::MIN)
    }
    fn step(&self, state: &mut AggregateState, arg: Value) -> Result<(), SqlError> {
        if let (AggregateState::Max(m), Value::Int(v)) = (state, arg) {
            if (v as f64) > *m {
                *m = v as f64;
            }
        }
        Ok(())
    }
    fn finalize(&self, state: &AggregateState) -> Value {
        match state {
            AggregateState::Max(m) if *m > f64::MIN => Value::Int(*m as i64),
            _ => Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    struct UpperFn;

    impl ScalarFunction for UpperFn {
        fn name(&self) -> &str {
            "upper"
        }
        fn arity(&self) -> Option<usize> {
            Some(1)
        }
        fn eval(&self, args: &[Value]) -> Result<Value, SqlError> {
            match args.first() {
                Some(Value::Text(s)) => Ok(Value::Text(s.to_uppercase())),
                _ => Err(SqlError::InvalidArgument),
            }
        }
    }

    #[test]
    fn 自定义标量函数_upper() {
        let f = UpperFn;
        assert_eq!(f.name(), "upper");
        assert_eq!(f.arity(), Some(1));
        let got = f.eval(&[Value::Text("abc".to_string())]).unwrap();
        assert_eq!(got, Value::Text("ABC".to_string()));
        assert!(f.eval(&[Value::Int(1)]).is_err());
        let builtin = BuiltinScalar;
        assert_eq!(
            builtin.eval(&[Value::Text("xyz".to_string())]).unwrap(),
            Value::Text("XYZ".to_string())
        );
    }

    #[test]
    fn 标量函数_lower_upper() {
        let lower = BuiltinScalar::get("lower").expect("lower exists");
        assert_eq!(lower.name(), "lower");
        assert_eq!(lower.arity(), Some(1));
        assert_eq!(
            lower.eval(&[Value::Text("ABC".to_string())]).unwrap(),
            Value::Text("abc".to_string())
        );
        assert!(lower.eval(&[Value::Int(1)]).is_err());

        let upper = BuiltinScalar::get("upper").expect("upper exists");
        assert_eq!(upper.name(), "upper");
        assert_eq!(
            upper.eval(&[Value::Text("abc".to_string())]).unwrap(),
            Value::Text("ABC".to_string())
        );
    }

    #[test]
    fn 标量函数_length_substr() {
        let length = BuiltinScalar::get("length").expect("length exists");
        assert_eq!(length.name(), "length");
        assert_eq!(length.arity(), Some(1));
        assert_eq!(
            length.eval(&[Value::Text("abc".to_string())]).unwrap(),
            Value::Int(3)
        );
        assert_eq!(
            length
                .eval(&[Value::Blob(alloc::vec![1u8, 2, 3, 4])])
                .unwrap(),
            Value::Int(4)
        );
        assert!(length.eval(&[Value::Int(1)]).is_err());

        let substr = BuiltinScalar::get("substr").expect("substr exists");
        assert_eq!(substr.name(), "substr");
        assert_eq!(substr.arity(), Some(3));
        assert_eq!(
            substr
                .eval(&[
                    Value::Text("hello".to_string()),
                    Value::Int(2),
                    Value::Int(3)
                ])
                .unwrap(),
            Value::Text("ell".to_string())
        );
    }

    #[test]
    fn 标量函数_abs_round() {
        let abs = BuiltinScalar::get("abs").expect("abs exists");
        assert_eq!(abs.name(), "abs");
        assert_eq!(abs.arity(), Some(1));
        assert_eq!(abs.eval(&[Value::Int(-5)]).unwrap(), Value::Int(5));
        assert_eq!(abs.eval(&[Value::Int(5)]).unwrap(), Value::Int(5));
        assert_eq!(abs.eval(&[Value::Float(-1.5)]).unwrap(), Value::Float(1.5));
        assert!(abs.eval(&[Value::Text("x".to_string())]).is_err());

        let round = BuiltinScalar::get("round").expect("round exists");
        assert_eq!(round.name(), "round");
        assert_eq!(round.arity(), Some(2));
        assert_eq!(
            round.eval(&[Value::Float(1.2345), Value::Int(3)]).unwrap(),
            Value::Float(1.235)
        );
        assert_eq!(
            round.eval(&[Value::Int(2), Value::Int(0)]).unwrap(),
            Value::Float(2.0)
        );
    }

    #[test]
    fn 标量函数_coalesce() {
        let coalesce = BuiltinScalar::get("coalesce").expect("coalesce exists");
        assert_eq!(coalesce.name(), "coalesce");
        assert_eq!(coalesce.arity(), None);
        assert_eq!(
            coalesce
                .eval(&[Value::Null, Value::Text("x".to_string()), Value::Int(1)])
                .unwrap(),
            Value::Text("x".to_string())
        );
        assert_eq!(
            coalesce.eval(&[Value::Null, Value::Null]).unwrap(),
            Value::Null
        );
        assert!(coalesce.eval(&[]).is_err());
    }

    #[test]
    fn 内置聚合函数_sum() {
        let func = BuiltinAggregates::get("sum").expect("sum builtin exists");
        let mut state = func.init();
        func.step(&mut state, Value::Int(1)).unwrap();
        func.step(&mut state, Value::Int(2)).unwrap();
        func.step(&mut state, Value::Int(3)).unwrap();
        assert_eq!(func.finalize(&state), Value::Int(6));

        let count = BuiltinAggregates::get("count").expect("count builtin exists");
        let mut cstate = count.init();
        count.step(&mut cstate, Value::Int(1)).unwrap();
        count.step(&mut cstate, Value::Int(2)).unwrap();
        assert_eq!(count.finalize(&cstate), Value::Int(2));
        assert!(BuiltinAggregates::get("missing").is_none());
    }
}
