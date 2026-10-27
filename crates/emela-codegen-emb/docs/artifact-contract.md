# 生成Cとビルドの契約

生成側はFlow IRからC・header・`emela_artifact.json`を返す。ビルド側は保存された一式、BSP、追加C／assembly、toolchain設定を入力にする。BSPの`build.py build`はEmela source、Core／Flow IR、RustのコンパイラーAPIを読まない。CLIのboard buildはこの2段階を続けて実行する。

```text
Core / Flow → C生成 → C + headers + emela_artifact.json
                                  ↓
                  BSP + 追加C + toolchain設定 → firmware
```

## manifest v1

生成directory直下に置く。CLIの最小出力は次の形になる。周辺機器を使う場合は必要なbridge headerも加わる。

```json
{
  "schema_version": 1,
  "abi_version": 1,
  "files": [
    {"path": "emela_embedded.h", "role": "public_header"},
    {"path": "emela_board.h", "role": "platform_header"},
    {"path": "emela_embedded.c", "role": "source"},
    {"path": "emela_embedded_exports.h", "role": "aliases_header"}
  ]
}
```

| role | 内容と必須条件 |
| --- | --- |
| `source` | C11 translation unit。`.c`で、1件以上。複数件を列挙でき、順序を保持してビルドへ渡す。 |
| `public_header` | `emela_embedded.h`。公開関数、生成value型、必要なretain／release等。必ず1件。 |
| `platform_header` | `emela_board.h`。必要なboard hook、task／event／defectの接続宣言。必ず1件。 |
| `aliases_header` | `emela_embedded_exports.h`。利用者名から生成symbolへの対応。C libraryでは省略できる。CLIは常に生成し、BSPは必須とする。 |
| `header` | 標準周辺機器bridge等の追加`.h`。必要なものだけを列挙する。 |

pathはmanifestからの正規化した相対pathとする。空path、絶対path、`.`／`..`成分、空成分、backslash、colon、NUL、重複pathを拒否する。Python readerは全列挙ファイルの存在と、symlink経由を含め生成directory内に収まることも確認する。未知field・未知role・重複key、必須roleの欠落、未対応バージョンも診断する。manifest自身は`files`に含めない。

両BSPは`source`だけを生成Cの入力とし、残存する`.c`をglobで追加しない。生成rootと列挙ファイルの親directoryをinclude pathへ入れ、BSP自身のsourceと`--source`で明示した利用者のsourceを加える。上表の3つのheader名は固定契約だが、生成Cのfilenameとtranslation unit数はbuilderへ埋め込まない。

## ABIの正本と変更方針

`schema_version`はmanifestの構造と検証規則、`abi_version`はC11での接続規約の版である。readerは対応する版だけを受け付ける。manifestの互換性を壊す変更はschemaの版を上げ、呼出し・所有権・hook等の互換性を壊す変更はABIの版を上げる。双方のreader、fixture、BSPも同時に更新する。

型のlayout、関数signature、String出力buffer、必要なhookと所有権は、その生成で出力したC headerと[外部C／ISR／ボードの契約](implementation.md#4-外部cisrボードとの境界)を正本とする。manifestには型配置や関数宣言を複製しない。ABI v1は別生成間でstruct layout・export番号やbinaryが一致するという意味ではない。Cとheaderは同じ生成の一式として使い、追加Cもそのheaderをincludeしてcompileする。公開symbolは`Output.exports`から取り、利用者の固定名はexport aliasで接続する。

生成物には固定heap／pool、scheduler等の必要なruntimeが含まれる。利用側は必要なplatform hookと外部symbolを実装し、retain／releaseと借用・所有の規約を守る。defect hookの即時停止、BSP entryの`fn() -> Int use Async`と`board_main_start`も既存契約に従う。manifestの検証成功だけではsymbol、資源容量、C signatureや意味論の適合を証明しない。compile／linkと動作試験も必要である。

## 生成側の利用方法

`compile`／`compile_flow`の`Output.files`にはmanifestも含まれる。そのまま全件を書き出す。追加の生成Cやalias headerを`Output.files`へ加える場合は、保存前に`Output::refresh_manifest()`を呼ぶ。CLIのdriver、`board_probe`、内部native試験もこの共通処理を使う。検証失敗時は既存の`Output.files`を書き換えない。

Rustの`artifact::Manifest::{from_files, from_json, validate}`と、BSP同梱の`artifact_manifest.py`が同じ形式を検証する。双方で同じ[正常fixture](../tests/fixtures/artifact/emela_artifact.json)と[拒否fixture](../tests/fixtures/artifact-invalid.json)を読む。

従来のC／headerだけをコピーしたdirectoryにはmanifestがないため、新しいbuilderは受け付けない。API／CLI／`board_probe`で一式を再生成する。生成結果を公開するときは全ファイルを揃え、manifestを最後に保存する。`board_probe`はstaging directoryで一式を準備して公開する。

## ビルド側の利用方法と鮮度

[board package](boards.md#cliを使わない生成とbuild)を展開した後は、生成一式とBSPを別directoryや別machineへ渡してビルドできる。追加C／assembly、projectのheader、toolchainも用意する。

```sh
python3 /path/to/bsp/build.py build \
  --generated-dir /path/to/generated \
  --project-dir /path/to/project \
  --output-dir /path/to/firmware \
  --optimization O2
```

このbuildにEmela sourceやRustのcheckoutは不要である。`--project-dir`はCのincludeとtoolchain作業directoryに使う。追加C／assemblyは`--source`で渡す。

CLIは[成功記録とhash照合](cli-embedded.md#build結果とflash時の照合)を維持する。manifest自身と列挙された全ファイルをhashに含め、nested pathも追跡する。Cのincludeによる未列挙headerや、新しいheaderによる置換も検出するため、生成directoryのheader一覧は保守的に照合する。未列挙の残存Cはコンパイルもhash追跡もしない。生成・ビルドの失敗は以前の成功記録を無効にする。

manifest自体は入力一覧で、内容hashは保持しない。CLIの成功記録が一式の鮮度を照合する。照合はCLIに同梱したreaderを使い、出力BSPのPython helperを検証前に実行しない。BSPを直接使うstandalone手順では、同じ生成の一式を渡す責任は呼出し側にあり、flashはfirmware成果物のhashを照合する。

## 個別に実行する試験

| 試験 | 入力と確認内容 |
| --- | --- |
| [Rustの生成契約](../tests/artifact.rs) | Flow→C／manifest、全ファイルの列挙、追加C／alias後の更新、共有拒否fixture。C compiler／BSP不要。 |
| [Pythonの契約・builder](../tools/test_artifact_contract.py) | 固定C一式を入力にし、readerと両BSPの実際のcompile引数を確認する。cross toolsをmock化してlink呼出しで止める。固定Cとplatform実装の接続はhost ccで`O0`／`O2`のcompile・実行も行う。 |
| [CLIの接続](../../emela-cli/tests/embedded_cli.rs) | CLIで生成後に元のEmela sourceを削除し、保存したmanifestをPythonで読み、host ccでcompile・実行する。 |
| [package](../tests/boards.rs) | 展開したBSPがcheckout外でmanifestを読み、未対応版をtoolchain起動前に拒否する。 |
| [鮮度](../tools/test_build_state.py)／[CLIのflash境界](../../emela-cli/tests/embedded_firmware_state.rs) | manifest、nested生成物、BSP reader、source等の変更をflasher起動前に拒否する。 |

```sh
cargo test -p emela-codegen-emb --test artifact --test boards --locked
python3 -B -m unittest discover -s crates/emela-codegen-emb/tools -p 'test_*.py'
cargo test -p emela-cli --test embedded_cli --test embedded_firmware_state --locked
```

これらは通常のCIで実行する。MCUの実命令、ELF配置・image生成、IRQ／実機動作は[ボードprobe](boards.md)で別に検証する。
