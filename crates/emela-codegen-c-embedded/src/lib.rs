//! 組み込み向け C バックエンドの作業用骨組み。
//!
//! モジュール分割は仮置き。ランタイムの動作や公開 API は設計メモで方針を決めてから選ぶ。

mod check;
mod const_eval;
mod emit;
mod names;
