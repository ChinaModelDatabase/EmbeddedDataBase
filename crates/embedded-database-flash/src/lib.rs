//! embedded-database 的闪存介质后端。
//!
//! 分层：
//! - `raw`：`RawFlash` trait（页/擦除块几何 + NOR 1→0 编程语义）。
//! - `sim`：`SimFlash` 内存模拟设备（坏块与故障注入，供测试）。
//! - `ftl`：擦除块级 FTL，对外提供 core 的 `BlockDevice`。
//!
//! 本 crate 为 `no_std`（需 `alloc`），不依赖任何第三方 crate。
//!
//! # 示例
//!
//! ```
//! use embedded_database_core::BlockDevice;
//! use embedded_database_flash::{Ftl, FtlConfig, SimFlash};
//!
//! let raw = SimFlash::new(64, 256, 4096);
//! let mut ftl = Ftl::new(raw, FtlConfig::default()).unwrap();
//! ftl.write_at(0, b"hello").unwrap();
//! let mut buf = [0u8; 5];
//! ftl.read_at(0, &mut buf).unwrap();
//! assert_eq!(&buf, b"hello");
//! ```
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod ftl;
pub mod raw;
pub mod sim;

pub use embedded_database_core::Error;
pub use ftl::{Ftl, FtlConfig, FtlStats};
pub use raw::{FlashError, FlashResult, RawFlash};
pub use sim::SimFlash;
