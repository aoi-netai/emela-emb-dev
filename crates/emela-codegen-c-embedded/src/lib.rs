//! Emela の組み込みC出力。
//!
//! 現在は GPIO の直線的な処理を、ヒープランタイムを使わずに C11 へ出力する。

mod emit;

use emela_core::Module;

/// 出力ファイル。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub main: String,
    pub gpio_header: String,
}

/// この初期バックエンドが出力できない機能。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
}

/// Core IR から、組み込み用の C11 ソースと GPIO 境界ヘッダーを生成する。
pub fn compile(module: &Module) -> Result<Output, Vec<Error>> {
    emit::compile(module)
}
