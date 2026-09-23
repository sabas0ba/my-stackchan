//! 実機と host テストで共有する表示・電源・通信処理。

#![no_std]

#[cfg(test)]
extern crate std;

pub mod actuators;
pub mod behavior;
pub mod model;
pub mod power;
pub mod renderer;
pub mod touch;
pub mod transport;
