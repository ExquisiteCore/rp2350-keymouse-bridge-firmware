//! SPDX-License-Identifier: MIT OR Apache-2.0
//!
//! Copyright (c) 2021-2024 The rp-rs Developers
//! Copyright (c) 2021 rp-rs organization
//! Copyright (c) 2025 Raspberry Pi Ltd.
//!
//! 配置 RP2350 链接脚本。

use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(rp2350)");

    // 把 memory.x 写到链接器能够搜索到的目录。
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    println!("cargo:rustc-link-search={}", out.display());

    std::fs::write(out.join("memory.x"), include_bytes!("rp2350.x"))
        .expect("write RP2350 memory.x");
    println!("cargo::rustc-cfg=rp2350");
    println!("cargo:rerun-if-changed=rp2350.x");
    println!("cargo:rerun-if-changed=build.rs");
}
