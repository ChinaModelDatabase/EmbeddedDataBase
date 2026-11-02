//! SQL 抽象语法树（AST）类型定义。

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// SQL 语句。
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Select(SelectStmt),
    Insert(InsertStmt),
    CreateTable(CreateTableStmt),
    DropTable(DropTableStmt),
}

/// SELECT 语句。
#[derive(Debug, Clone, PartialEq)]
pub struct SelectStmt {
    pub columns: Vec<SelectItem>,
    pub from: Option<String>,
    pub joins: Vec<JoinClause>,
    pub where_clause: Option<Expr>,
    pub order_by: Vec<OrderByItem>,
    pub limit: Option<i64>,
    pub group_by: Vec<String>,
    pub having: Option<Expr>,
}

/// JOIN 子句。
#[derive(Debug, Clone, PartialEq)]
pub struct JoinClause {
    pub table: String,
    pub alias: Option<String>,
    pub on_left: String,
    pub on_right: String,
}

/// SELECT 列项。
#[derive(Debug, Clone, PartialEq)]
pub enum SelectItem {
    Wildcard,
    Column(String),
    Aggregate(AggregateFn),
}

/// 聚合函数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateFn {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

/// INSERT 语句。
#[derive(Debug, Clone, PartialEq)]
pub struct InsertStmt {
    pub table: String,
    pub columns: Vec<String>,
    pub values: Vec<Vec<Expr>>,
}

/// CREATE TABLE 语句。
#[derive(Debug, Clone, PartialEq)]
pub struct CreateTableStmt {
    pub table: String,
    pub columns: Vec<ColumnDef>,
}

/// 列定义。
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    pub col_type: String,
    pub primary_key: bool,
    pub not_null: bool,
}

/// DROP TABLE 语句。
#[derive(Debug, Clone, PartialEq)]
pub struct DropTableStmt {
    pub table: String,
}

/// 表达式。
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Literal(Literal),
    Column(String),
    BinaryOp(Box<Expr>, BinOp, Box<Expr>),
    UnaryOp(UnOp, Box<Expr>),
    IsNull(Box<Expr>, bool),
    Aggregate(AggregateFn),
}

/// 字面量。
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Integer(i64),
    Float(f64),
    StringLit(String),
    Null,
}

/// 二元运算符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Like,
}

/// 一元运算符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
}

/// ORDER BY 项。
#[derive(Debug, Clone, PartialEq)]
pub struct OrderByItem {
    pub expr: Expr,
    pub asc: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ast_创建_statement() {
        let stmt = Statement::DropTable(DropTableStmt {
            table: "users".into(),
        });
        assert!(matches!(stmt, Statement::DropTable(_)));
    }

    #[test]
    fn ast_表达式() {
        let e = Expr::BinaryOp(
            Box::new(Expr::Column("x".into())),
            BinOp::Add,
            Box::new(Expr::Literal(Literal::Integer(1))),
        );
        assert!(matches!(e, Expr::BinaryOp(_, BinOp::Add, _)));
    }
}
