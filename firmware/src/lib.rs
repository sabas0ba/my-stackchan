//! 実機と host テストで共有する表示・電源制御。

#![no_std]

#[cfg(test)]
extern crate std;

pub mod power;
pub mod renderer;
