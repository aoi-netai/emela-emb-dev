# Flow IR 組み込み C backend の対応範囲

この文書は、実装計画のフェーズ0として、組み込みC backendの初期対応範囲を決める。ここでの「対応」は実装済みという意味ではなく、初期版で目指す契約である。

## 実装境界

- 入力は単相化前の `emela_core::Module` とする。backend内で入口から到達する関数を単相化し、`emela_lir::from_core` でFlow IRを作る。
- Flow IRの構造体を直接読む。`emela_lir::dump` が出すテキストは調査用とし、入力として解析しない。
- `Stage::Flow` の検証後、Flow IRから先は独立crate `emela-codegen-emb` の組み込み専用変換と出力器で処理する。LIRのHeap IRパス `to_heap` は呼ばない。
- `const` はbackend内のFlow IR評価器で静的データにする。評価できないconstは利用箇所を示して拒否する。評価結果がdefectの場合はビルドを失敗させず、参照されたときにdefect hookを呼ぶ。
- 初期実装では `emela-core`、`emela-syntax`、`emela-lir` の変更を要件にしない。既存の `@external(c, ...)` とbackendの設定で足りない機能は、初期版では診断付きで対象外にする。
- ホスト向けC backendの生成物とABIは変更しない。

現行Flow IRの定義は [`emela-lir/src/ir.rs`](../../emela-lir/src/ir.rs)、builtin一覧は [`emela-core/src/intrinsics.rs`](../../emela-core/src/intrinsics.rs) にある。将来それらにvariantが追加された場合、組み込みbackendの対応表も更新する。

## 対応状態の意味

| 状態 | 意味 |
| --- | --- |
| 対応 | 初期版でサポートする。設定されたRAM・Flash・stack・poolの上限に達した場合のdefectは、機能未対応とは区別する。 |
| 制約付き | 条件を満たす場合にサポートし、条件外は利用箇所を示してコンパイルエラーにする。 |
| 未対応 | 初期版ではサポートせず、利用箇所を示してコンパイルエラーにする。 |
| Flow IRでは不正 | 正常な `Stage::Flow` には出ないHeap IR専用の要素。見つけた場合は利用者のエラーでなくコンパイラ内部エラーにする。 |

## 実装基盤の分類

各variantについて、初期版の状態とは別に、どの実装基盤があればサポートできるかを示す。ここでの「可能」はメモリ容量内に必ず収まるという意味ではなく、必要な実装方式の分類である。

| 分類 | 意味 |
| --- | --- |
| 静的に対応可能 | 値・呼び出し先・制御フローをコンパイル時に確定でき、実行時heapを使わずに生成Cの値・スタック・定数データで処理できる。小さな生成runtime helperやboard hookは含む。 |
| ヒープで対応可能 | 固定容量heapによる実行時確保・解放があれば処理できる。容量は設定値で上限を設け、ISRからは利用しない。 |
| 独自ランタイムで実装可能 | allocatorだけでは足りず、動的dispatch、handler/evidence管理、fiber schedulerなどの実行機構を追加すれば処理できる。必要に応じてheapも併用する。 |
| Flow IR対象外 | Heap IR専用variantなど、正常なFlow IRに現れないもの。 |

分類欄に複数の方式がある場合は、値の形や静的特殊化の成否によって必要な基盤が変わる。独自ランタイムの追加は、core・syntax・LIRを変更せず `emela-codegen-emb` 内で生成コードと補助runtimeを実装する想定であり、それだけでISR安全やリアルタイム性が保証されるわけではない。対応へ上げたvariantは、設定したメモリ量で全ての言語動作を実装できるという意味であり、実装量が小さいという意味ではない。既存のホスト向けC backendは `emela_lir::to_heap` 後のIRを出力するため、組み込み経路ではclosure変換・defect/suspend処理・所有権管理をbackend内に実装する必要がある。既存の「状態」欄は初期版で目指す範囲を表し、この分類とは別に扱う。

メモリ規模の目安はNUCLEO-F446RE（STM32F446RE: SRAM 128 KB、Flash 512 KB）とRaspberry Pi Pico 1（RP2040: SRAM 264 KB、基板上QSPI Flash 2 MB）とする。どちらも小規模な固定heapや選択式runtimeを検討できるが、全variantや任意のアプリが収まる保証ではない。F446REはSRAMの余裕がより小さく、Pico 1で複数コアから同じheapを使う場合は同期が別途必要になる。最終的な容量は生成物のlink map、stack、String容量、heap上限で確認する。[STM32F446RE仕様](https://www.st.com/en/microcontrollers-microprocessors/stm32f446re.html)、[Raspberry Pi Pico仕様](https://www.raspberrypi.com/products/raspberry-pi-pico/)。

## 型とメモリの基本方針

- `Int` / `Bool` は32 bit、`Int64` は64 bit、`Float` は倍精度、`()` / `Never` は実行時の値を持たない。
- 引数なしenumは小さな整数で表す。固定レイアウトにできるenum・タプル・レコードはC構造体として値渡しする。フィールドがListなどのheap参照でも、構造体自体はinline値にし、参照先だけheapに置く。再帰型は再帰する辺をheap上の箱への参照にする。これはbackend独自の型表で決め、LIRの `Repr` は変更しない。
- `String`値自体には固定長配列を埋め込まず、UTF-8データへのpointerとbyte長を持つ軽量viewにする。viewのbacking ownerが必要な場合は、その参照もdescriptorまたはbackendの所有権情報で追跡する。リテラルは読み取り専用データを参照し、不要なコピーを避ける。生成箇所ごとに最大byte長を解析し、既知の上限に合わせてバッファーを確保する。連続するString連結は中間Stringを作らず最後に一度だけ書き出し、非escapeかつ短命な一時値はstack scratch上で生存期間に応じて領域を再利用する。stack scratchの予算を超える値、再帰呼び出しで大きくなる値、長寿命の値はcaller領域・固定heap/pool・fiber frameへ配置する。最大長が静的に確定しない値には `string_capacity`（既定256 byte、設定上限512 byte）を上限として適用し、超過は切り詰めずdefectにする。Unicode grapheme処理はEmela仕様に対応する固定versionのテーブルを使う。
- Listノードと再帰型の再帰する辺は固定領域のheapに置く。tupleやrecordなど固定レイアウトの値そのものは、heap参照を含んでいてもinline値にする。`heap_bytes = 0` はheapを使わないプロファイルであり、heapが必要なプログラムはheap容量を指定すればサポートする。正の容量を指定した場合は、`malloc`を使わない固定heapを出力する。heap箱は型ごとの構造体と参照カウントを持ち、サイズ別の空きリストで再利用する。Flow IRには `Dup` / `Drop` がないため、所有権解析とretain/releaseの挿入は `emela-codegen-emb` 内で行う。inline値に含まれるheap参照も所有権解析の対象とする。最後の参照の解放は再帰せず、解放待ちリストを使う。容量不足はdefectにする。
- Runtimeの固定領域はList・再帰値だけでなく、動的closure/evidence/handler、escapeしたString backing、Fiber・Deferred・suspend継続にも使う。用途別poolと上限を設定できるようにし、pool不足はdefectにする。フェーズ7のfiber/frame/task/event poolは固定heapと別領域とし、scalarだけのsuspend経路ではheapを必要としない。ISRからこれらの確保・解放を行わない。ISRが使う完了通知APIは、確保済みtaskに対応する固定event queueへ値をcopyする操作に限定する。
- ISRから到達する関数ではheap確保・解放・参照管理、再帰、suspend関数を禁止する。ISRから呼ぶ外部C関数は `Options` でISR安全と明示されたものに限る。ISRのスタック総量は初期版では見積もらない。
- `@external type` は不透明な `void *` として値で保持する。`Any`と`EmelaError`はタグ付きdescriptorを共有し、tag 0の一般値boxと正のtagのerror payloadを区別する。`Any`への変換では、値型・外部handleをbackend独自の型記述付きboxに入れ、取り出し時は静的に既知の型へ戻す。heap参照型はその参照を保持し、動的関数・handler・evidenceも含めて所有権を管理する。
- `Function`, `Handler`, `Evidence` は静的特殊化できる場合は静的に処理し、動的値として残る場合は固定heap上のclosure・handler・evidence表として保持する。動的dispatchと解放後のhandler検査は生成runtimeが行う。
- `Cell`は固定heapのtyped contentを所有参照で共有し、creator frameから独立した寿命を持つ。sourceのcapability制約は維持し、そこで許されないescapeをheap化だけで許可しない。明示所有参照によるCell/closureの寿命延長と、Stateを自動global保存したりISRと共有したりする`@static`相当のAPIは区別する。後者は提供しない。

## Flow IR `Value` の対応表

| Variant | 意味 | 状態 | 実装基盤 | 初期版の方針 |
| --- | --- | --- | --- | --- |
| `Const` | 定数値 | 対応 | 静的に対応可能 | Int、Int64、Float、Bool、String、UnitをC定数または読み取り専用データにする。 |
| `Prim` | 算術演算・比較 | 対応 | 静的に対応可能 | 算術・比較をCへ出す。Cの未定義動作を避け、Emelaの整数オーバーフロー、ゼロ除算、シフトの意味を保つ。Floatの剰余は`fmod`を使い、利用側はlibmをlinkする。String比較は内容で比較する。 |
| `Call` | 関数またはhandler操作の呼び出し | 対応 | 静的（直接呼出し）／ヒープ＋独自ランタイム（動的呼出し・suspend） | 直接・間接・操作呼び出しを扱う。`fails`は関数ごとのエラー出力引数で扱い、suspend呼び出しは生成state machineとruntime schedulerで継続する。外部C callbackのABI条件は「外部C・ボード境界」に従う。 |
| `Builtin` | 言語組み込み関数 | 対応 | 個別builtin表を参照 | 正常なFlow IRに現れる全builtinを下の表のとおり扱う。 |
| `Make` | enum構成子による値の生成 | 対応 | 静的（inline値）／ヒープ（再帰辺） | 固定レイアウトのenumは、heap参照を含むフィールドもポインターとして持つC構造体にする。再帰する辺の生成には固定heapを使う。heap容量不足はdefectにする。 |
| `Field` | enum値からフィールドを取得 | 対応 | 静的またはヒープ（enumの格納表現による） | inline値またはheap箱からフィールドを読む。型・構成子・フィールド番号の整合性はFlow IR検証で確認する。heap値の確保条件とISR制約は共通のメモリ方針に従う。 |
| `Tag` | enum構成子の番号を取得 | 対応 | 静的またはヒープ（enumの格納表現による） | inline値またはheap箱から構成子tagを読む。heap値の利用に関するISR制約は共通のメモリ方針に従う。 |
| `Tuple` | タプル値を生成 | 対応 | 静的に対応可能 | 固定レイアウトのタプルをC構造体にする。Listなどのheap参照を含む場合もタプル自体はinline値とし、フィールド位置とUnit placeholderを保つ。 |
| `TupleGet` | タプル要素を取得 | 対応 | 静的に対応可能 | C構造体から要素を読み、heap参照を含むmanaged valueはborrowed fieldからretainする。 |
| `List` | Listを生成 | 対応 | ヒープで対応可能 | scalar、List参照、List参照を含むTuple/enumを要素にできる。構成時の参照保持とnode破棄時の解放を行う。実行時生成に固定heapを使い、`heap_bytes = 0`ではheapが必要な入力を診断する。容量不足はdefectにし、ISR内ではheap確保を禁止する。 |
| `ListIsEmpty` | Listが空か判定 | 対応 | ヒープで対応可能（heap上のListを参照） | List表現に従って空か判定する。Listの生成とheap参照に関する条件は共通のメモリ方針に従う。 |
| `ListHead` | Listの先頭要素を取得 | 対応 | ヒープで対応可能（heap上のListを参照） | 非空Listから要素を読む。非空条件はFlow IR側のパターン制御で保証する。Listの生成とheap参照に関する条件は共通のメモリ方針に従う。 |
| `ListTail` | Listの先頭を除いた残りを取得 | 対応 | ヒープで対応可能（heap参照を返す） | 末尾を返し、参照カウントを維持する。heap参照に対するretain/releaseはbackend内で挿入する。Listの生成とheap参照に関する条件は共通のメモリ方針に従う。 |
| `FnRef` | トップレベル関数への参照 | 対応 | 静的（既知の関数）／独自ランタイム（動的関数値） | 静的参照と動的関数値の両方を扱い、動的呼び出しは共通dispatcherへ渡す。 |
| `Closure` | 関数本体とcaptureを持つclosure | 対応 | 静的（特殊化可能なclosure）／ヒープ＋独自ランタイム（動的closure） | 入れ子closureをbackend内で持ち上げ、capture環境の生成・保持・解放と動的呼び出しを扱う。戻り値・データ構造内での持ち回りも扱う。 |
| `MakeClosure` | heap上のclosure値を生成 | Flow IRでは不正 | Flow IR対象外 | Heap IR専用。出現したら内部エラー。 |
| `EnvGet` | closure環境からcaptureを取得 | Flow IRでは不正 | Flow IR対象外 | Heap IR専用。出現したら内部エラー。 |
| `EvLookup` | 証拠表からeffect handlerを検索 | 対応 | 静的（特殊化）／ヒープ＋独自ランタイム（動的証拠表） | 静的特殊化と動的証拠表の両方を扱う。動的表は追加順を保ち、後のbindingを優先する。 |
| `EvExtend` | 証拠表にhandlerを追加 | 対応 | 静的（特殊化）／ヒープ＋独自ランタイム（動的証拠表） | 静的特殊化に加えて、動的な証拠表を固定heap上に構築する。 |
| `Handler` | effect操作を実装するhandler値 | 対応 | 静的（特殊化）／ヒープ＋独自ランタイム（動的handler） | 動的handlerと操作closureを保持し、`Retire`後の呼び出しをdefectにする。handler値の保存・返却も扱う。 |
| `Fiber` | 並行処理・fiber操作 | 対応 | 独自ランタイム（scheduler・再開状態） | `Scope`、`Fork`、`Deferred`、`Join`、`Await`、`Interrupt`、`Attempt`、`Complete`、`DeferredAwait`、`Poll`、`CauseKind`、`CauseToString`、`CauseRaise`をcooperative schedulerとstate machineで扱う。fiber数・frame容量の上限は設定し、上限到達はdefectにする。 |
| `IsError` | エラーが指定enumか判定 | 対応 | 静的に対応可能 | `EmError`のenumタグを調べる。 |
| `AsError` | エラーを指定enumの値として取得 | 対応 | 静的に対応可能 | `IsError`成功後に該当error enumの値として取り出す。 |
| `Runtime` | 言語組み込み以外のruntime操作 | 対応 | RuntimeOp表を参照 | Flow段階で許すoperationをすべて扱う。Heap IR専用operationは引き続き内部エラー。 |
| `Coerce` | 値の型・effect表現を調整 | 対応 | 静的（同一ABIまたは生成wrapper）／ヒープ（Any box） | function effect flagを調整し、値型・外部handle・closure・handler・evidenceを`Any`にbox/unboxする。unbox先は型検査済みの静的型とし、retain/releaseはbackend内で扱う。`EmError`への変換は通常のAny変換と混ぜない。 |

### Builtinごとの対応

`Value::Builtin` が扱うbuiltinは次のとおり。`Panic`、`Todo`、`Dbg`は正常なFlow IRでは別の形へlowerされるため、`Value::Builtin`として現れた場合は内部エラーにする。

| Builtin | 意味 | 状態 | 実装基盤 | 初期版の方針 |
| --- | --- | --- | --- | --- |
| `StringLength`, `StringChars` | 書記素数を数える・書記素ごとのListを作る | 対応 | 静的（StringLength、固定Unicode table）／ヒープ（StringCharsのList結果） | 固定したUnicode grapheme tableに基づき全Unicode入力を扱う。`StringChars`のList要素は元Stringのデータを指すviewとし、Listの生存中は参照元データの寿命を保つ。Listは固定heapに作る。 |
| `StringByteSize` | UTF-8 byte数を数える | 対応 | 静的に対応可能 | UTF-8 byte数を返す。 |
| `StringAppend`, `StringContains`, `StringStartsWith`, `StringEndsWith`, `StringTrim` | 文字列を連結・検索・前後判定・trim | 対応 | 静的に対応可能（解析済み容量のbufferまたはview） | 連続appendは結合先へ一度だけ書き、検索・前後判定はviewを直接読む。`trim`は入力内のviewを返せる場合はコピーしない。必要な出力bufferはbyte長上限と生存期間から決める。White_Spaceは仕様の集合を扱う。 |
| `StringSplit` | separatorでStringをListに分割 | 対応 | ヒープで対応可能（List結果） | 空でないseparatorと空separatorを扱う。空separatorは言語仕様のgrapheme境界に従う。List要素は元Stringを指すviewとし、Listの生存中は元データを保持する。Listノードは固定heapに作る。 |
| `StringJoin`, `StringCodePoints` | Listを連結・Unicode code pointのListを作る | 対応 | ヒープで対応可能（Listを扱う） | 任意のList入力・結果を扱う。joinの出力上限はList長と要素長の既知上限から解析し、分からない場合は `string_capacity` を適用する。CodePointsのListノードは固定heapに作る。 |
| `StringFromCodePoint` | code pointからStringを作る | 対応 | 静的に対応可能 | Unicode scalar valueかを確認し、Option[String]を返す。 |
| `IntToString`, `IntParse`, `IntCheckedAdd` | Intの文字列化・parse・overflow検査付き加算 | 対応 | 静的に対応可能 | Int幅から得られるbyte長上限のbuffer/viewを使う。parse形式は仕様に従う。 |
| `IntBitAnd`, `IntBitOr`, `IntBitXor`, `IntBitNot` | 32 bit整数のbit演算 | 対応 | 静的に対応可能 | 32 bit整数演算。 |
| `IntShiftLeft`, `IntShiftRight`, `IntShiftRightUnsigned` | 32 bit整数のshift | 対応 | 静的に対応可能 | 負のシフト量はdefect。範囲外シフトも仕様どおり明示処理し、Cの未定義動作にしない。 |
| `Int64FromInt`, `Int64ToInt`, `Int64ToString` | Int/Int64変換・Int64の文字列化 | 対応 | 静的に対応可能 | `Int64ToInt`は仕様どおり下位32 bitを保つ。String化は容量超過をdefectにする。 |
| `Int64BitAnd`, `Int64BitOr`, `Int64BitXor`, `Int64BitNot` | 64 bit整数のbit演算 | 対応 | 静的に対応可能 | 64 bit整数演算。 |
| `Int64ShiftLeft`, `Int64ShiftRight`, `Int64ShiftRightUnsigned` | 64 bit整数のshift | 対応 | 静的に対応可能 | 32 bit shift量に対し仕様の境界処理を行う。 |
| `FloatToString`, `FloatParse`, `FloatFromInt` | Floatの文字列化・parse・Intから変換 | 対応 | 静的に対応可能（仕様準拠実装） | Emela仕様に合う文字列形式・parse規則を実装する。Float書式の最大byte長を上限として出力領域を計画する。toolchain設定で足りない場合はcodegen runtime helperを使い、library差で機能を拒否しない。 |
| `FloatFloor`, `FloatCeil`, `FloatRound`, `FloatTruncate` | Floatの整数丸め | 対応 | 静的に対応可能（libmまたは同等helper） | toolchainのlibmをlinkするか、同じ規則のhelperを出力する。 |
| `Panic`, `Todo` | defectを発生させる | Flow IRでは不正 | Flow IR対象外 | 正常なFlow IRでは `Term::Defect` になる。 |
| `Dbg` | 値をdebug出力して返す | Flow IRでは不正 | Flow IR対象外 | 正常なFlow IRでは `RuntimeOp::DebugPrint` になる。 |
| `Inspect` | Showを満たす値のdebug文字列を返す | Flow IRでは不正 | Flow IR対象外 | 通常はShow関数と`RuntimeOp::ShowString`へlowerされる。Stringは最上位でもquoteする。 |
| `ShowNested` | aggregate内の要素をShow表示する | Flow IRでは不正 | Flow IR対象外 | monoで`Show.show`呼出しかString専用の`ShowString`へ置換する。通常Flowには残らない。 |
| `ShowString`, `DebugPrint` | quote付きString化・表示済みStringのdebug出力 | Flow IRでは不正 | Flow IR対象外 | lowering内部Builtin。対応する`RuntimeOp::ShowString`・`RuntimeOp::DebugPrint`へ変換する。 |
| `CellNew`, `CellGet`, `CellSet` | State.run用のCellを生成・読取・更新 | 対応 | ヒープで対応可能（共有Cell） | 固定heap上のtyped Cellを所有参照で共有し、Stringのsnapshot、capture、suspend、引数・結果を扱う。cycleが作れる型を含むmoduleだけに回収処理を生成する。局所Cellの静的特殊化は将来の最適化候補。 |

## Flow IR `Stmt` の対応表

| Variant | 意味 | 状態 | 実装基盤 | 初期版の方針 |
| --- | --- | --- | --- | --- |
| `Let` | 値を評価して変数へ束縛 | 対応 | 静的に対応可能 | ANFの評価順を保って値を束縛する。Flow段階では `on_defect` と `on_suspend` はともに `None` でなければならない。 |
| `If` | 条件で2つのsequenceを選ぶ | 対応 | 静的に対応可能 | 条件はInt32/Boolとして分岐し、各armの `Yield` で結果を束縛する。 |
| `Switch` | enum tagまたはliteralで分岐 | 対応 | 静的に対応可能 | enum tagおよびInt32/Int64 literal switchを出す。文字列patternはFlowの比較式として処理する。defaultなしtag switchはenumの全variantを覆うことを検証する。 |
| `Loop` | loop変数を持つ繰り返し | 対応 | 静的に対応可能 | 初期値をloop変数へ束縛し、`Recur`をloop継続、`Yield`をloop脱出にする。 |
| `Block` | label付きの脱出可能な範囲 | 対応 | 静的に対応可能 | labelごとの脱出先を生成し、`Break`と失敗landingの制御先に使う。 |
| `Protect` | 本体の終了時にcleanupを実行 | 対応 | 静的に対応可能 | 正常終了、`break`、`return`、`fail`の各経路でcleanupを実行する。`recur`が保護範囲を出ない場合はcleanupしない。ターゲット方針によりdefectではcleanupせず即時停止する。 |
| `Dup` | heap参照を複製 | Flow IRでは不正 | Flow IR対象外 | 明示的参照カウントはHeap IR専用。 |
| `Drop` | heap参照を解放 | Flow IRでは不正 | Flow IR対象外 | 明示的参照カウントはHeap IR専用。 |

## Flow IR `Term` の対応表

| Variant | 意味 | 状態 | 実装基盤 | 初期版の方針 |
| --- | --- | --- | --- | --- |
| `Yield` | compound statementの値を渡して正常終了 | 対応 | 静的に対応可能 | compound statementの結果をdestinationへ渡す。 |
| `Return` | 関数から値を返す | 対応 | 静的に対応可能 | C関数から値またはvoidを返す。heap値がある場合は所有権を呼び出し側へ移す。 |
| `Fail` | error値を返して失敗 | 対応 | 静的に対応可能 | `EmError`を書き込み、失敗チャネルで関数を戻る。 |
| `Break` | label付きBlockから値を返して脱出 | 対応 | 静的に対応可能 | 対応する`Block`の脱出先へ値を渡す。cleanupを通す。 |
| `Recur` | loopの次の反復へ進む | 対応 | 静的に対応可能 | loopの引数を更新して先頭へ戻る。 |
| `Defect` | 回復不能な異常で停止 | 対応 | 静的に対応可能 | `emela_board_defect`を呼ぶ。戻らない。defect時にcleanupやheap解放は行わない。 |
| `Unwind` | Heap IRの保留defectを呼び出し元へ返す | Flow IRでは不正 | Flow IR対象外 | Heap IR専用。出現したら内部エラー。 |
| `Unreachable` | 到達不能であることを示す | 対応 | 静的に対応可能 | Cのunreachable表現または停止経路を出し、後続処理を置かない。 |

## 失敗・defectの契約

- `fails`は回復可能な制御経路として扱う。失敗する関数は最後の引数に `EmError *` を受け、失敗値をそこへ書いて戻る。呼び出し側は各`OnFail::Break` / `OnFail::Propagate`を明示的な分岐へlowerする。
- `EmError`は到達可能なerror enumを識別できるタグ付き共用体とする。失敗が`main`または公開入口まで届いた場合はerror payloadを表示用Stringに変換せず、`EMELA_DEFECT_UNHANDLED_ERROR` と静的な `unhandled error` messageをdefect hookへ渡す。
- `Term::Defect`は通常の失敗と混ぜない。グローバルな保留領域や巻き戻しを行わず、defect kind・message・lengthを `emela_board_defect` に渡して停止する。
- defectは`Protect`のcleanupを実行しない。C側のdefect hookは出力の停止、安全な状態への移行、停止またはresetを担当する。
- defectでcleanupを省略するのは、通常のEmela意味論から外れる組み込みターゲット固有の制約である。
- Cで未定義動作になるInt演算、シフト、Float変換などは小さなruntime helperでEmelaの規則に合わせる。

## 並行処理とsuspend

- `Value::Fiber`の全operationを、codegen内で生成するcooperative schedulerとstate machineで実装する。fiber、Deferred、Cause、suspend継続の個数・frame容量は固定poolで制限し、上限到達をdefectにする。
- schedulerは通常処理側だけで動かす。ISRからfiberを起動・再開せず、ISRは固定長event queueへ通知し、main側でeventをschedulerへ渡す。schedulerのyield budgetとboard idle hookを使って通常処理側の実行時間を調整する。
- `suspend fn`外部関数は生成 `EmelaTask` ABIで開始し、外部C実装が完了eventを通知したときに再開する。blocking関数をsuspend関数として直接呼ぶ扱いにはしない。ISRから到達するsuspend関数は禁止する。
- handlerのsuspend操作は、Flow IRに残るresumeの回数や保存先を制限せず、heap上の継続とstate machineでEmelaのresume意味論を実装する。
- Flow IRの `RuntimeOp::Hold` / `Unhold` はhardware ISRのmaskではなくfiberのinterrupt/cancelを延期・再開する操作として実装する。

## 外部C・ボード境界

- 外部関数は既存Coreにある `@external(c, ...)` bindingを使う。Emelaの関数値をC callback引数・戻り値へ自動変換するthunkは、保持期間と呼出し元の契約が未指定のため、この計画では未対応とする。boardが実装するtyped task producer/releaseはmainから呼ぶ明示的なC callback契約であり、Emela関数値の自動thunkとは別である。
- suspend外部関数用に`EmelaTask`と完了通知ABIを`emela_embedded.h`に宣言する。scalarは値通知、String/tuple/enum/List/Any等は型付きproducer/context leaseを使う。署名へEmela関数値やruntime capabilityを直接含む形は診断する。C側は完了時に結果または失敗をtaskへ通知し、schedulerはmain側で所有権を取得して継続を再開する。ISRは固定長event queueへ通知するだけでproducerもEmela callbackも直接実行しない。source externalの非fails契約は維持する。
- 同期String externalには、ホストC向け関数との誤linkを防ぐsymbol/借用・所有/出力bufferのoverride設計が必要で、現状は診断する。suspendのString externalは`Options::with_task_adapter`による明示mappingとtyped task producer ABIで扱う。
- `Options`にはboard name、ISRとして扱う公開関数、外部関数override（symbol、String ABI、ISR安全指定、suspend task adapter）、`heap_bytes`、`string_capacity`、Fiber/frame/eventの各pool上限を渡せるようにする。TOML読み込みとCLIからの選択はcodegen外の後続フェーズとする。
- 生成する `emela_embedded.h` は公開Emela入口、外部関数ABI、`EmelaTask`の完了通知ABIを宣言する。`emela_board.h` は必要なhookだけ宣言する。
- 必須hookは `_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length)`。kindは `EMELA_DEFECT_PANIC`、`EMELA_DEFECT_TODO`、`EMELA_DEFECT_DIVISION_BY_ZERO`、`EMELA_DEFECT_NO_MATCH`、`EMELA_DEFECT_FLOAT_TO_INT`、`EMELA_DEFECT_NEGATIVE_SHIFT`、`EMELA_DEFECT_HEAP_EXHAUSTED`、`EMELA_DEFECT_STRING_CAPACITY`、`EMELA_DEFECT_UNSUPPORTED_UNICODE`、`EMELA_DEFECT_UNHANDLED_ERROR`、`EMELA_DEFECT_ASSERT`を区別する。messageはUTF-8 byte列で、NUL終端を保証せず、hook呼び出し中だけ有効とする。hookはISRから呼ばれる場合もあるため、ISR安全でなければならない。
- `main`を出力する場合は `void emela_board_init(void)` を起動時に呼び、Emelaのmain終了後は `void emela_board_idle(void)` を繰り返す。`DebugPrint`を使う場合は `void emela_board_debug(const char *bytes, uint16_t length)` を要求する。Consoleなどのproject effectは個別の外部C関数として解決し、標準のboard hookには含めない。
- `emela_board_debug`にはNUL終端しないUTF-8 byte列とbyte長を渡す。hookは戻る前に読み切るか自分の領域へcopyし、改行を付けて一行を出力する。ISRから到達する場合はISR安全に実装する。
- 初期版はSTM32 HALの自動adapterを出力しない。GPIO/UART/I2C/SPIなどの操作は、プロジェクトのeffect定義とC外部関数を通じてボード側が実装する。HAL adapterとboard TOMLの自動生成は後続の拡張とする。
- 初期版はEmelaソースの新しい標準module、`@external(embedded, ...)`、`@static`を追加しない。Cellやclosureの寿命は明示的な所有参照で延長できるが、ISRからの利用や自動global State保存APIは追加しない。

## `RuntimeOp` のFlow段階対応

| Operation | 意味 | 状態 | 実装基盤 | 方針 |
| --- | --- | --- | --- | --- |
| `ShowString` | Stringを表示用にescape | 対応 | 静的に対応可能 | quoteで囲み、`\"`、`\\`、`\n`、`\t`、`\r`、`#{`の前の`\#`、その他の制御byteを仕様どおりescapeする。動的出力は`string_capacity`内のscratchへ書き、超過はdefectにする。 |
| `DebugPrint` | debug用に一行出力 | 対応 | 静的に対応可能 | board hookへNUL終端しないUTF-8 byte列と長さを同期で渡す。hookは呼び出し中に読み切るか自分の領域へcopyし、改行を付ける。ISRから到達する場合はISR安全なhookを使う。 |
| `Retire` | handlerの使用を終了 | 対応 | 静的（handler消去）／独自ランタイム（動的handler） | 静的に消せる場合は消去する。動的handlerはretired状態を保持し、その後の操作呼び出しをdefectにする。 |
| `Hold`, `Unhold` | fiber interruptの延期・再開 | 対応 | 独自ランタイム（interruptの延期） | 現在のfiberでreleaseがsuspendしている間、fiber interrupt/cancelを延期し、対応する`Unhold`で再開する。hardware ISRはmaskしない。 |
| `Defect`, `DefectTake`, `DefectResume`, `Yield`, `YieldPerform` | defect伝播・suspend制御 | Flow IRでは不正 | Flow IR対象外 | Heap IR専用operation。出現したら内部エラー。 |

## 設定・境界によって拒否する機能

次は実行構成または外部ABIの制約であり、該当箇所を示してコンパイルエラーにする。

- `heap_bytes = 0`で、実行時heap確保が必要なList・再帰値・closure・Fiber等が残る。この場合はheapを有効にする構成を選ぶ。
- ISRからheap値、String external、blocking/suspend関数、再帰関数に到達する。
- Emela関数値をC関数pointerへ自動変換して外部C関数との境界で受け渡す。
- `@static`相当の入口間で共有する永続Stateを要求する。

外部C callbackの自動thunkと、入口間のStateを自動保存するAPIは、この対応表の初期対応には含めない。利用者が所有参照を保持するCell/closureの共有とは区別する。Flow IRでは表せるが外部ABI制約に該当する項目は利用者向け診断にする。一方、Heap IR専用variantが `Stage::Flow` に現れた場合だけ内部エラーにする。

## フェーズ0の完了条件

- 現行 [`Value`](../../emela-lir/src/ir.rs)、[`Stmt`](../../emela-lir/src/ir.rs)、[`Term`](../../emela-lir/src/ir.rs) の全variantに方針がある。
- `Builtin`と`RuntimeOp`の全variantに方針がある。
- 型の値表現・heap表現、割り込み制約、失敗とdefect、外部C境界の契約が定まっている。
- 未対応項目は、backendの診断として拒否する方針が定まっている。

この分類では、正常なFlow IRに現れる全variantを「対応」とする。`MakeClosure`、`EnvGet`、`Dup`、`Drop`、`Unwind`などHeap IR専用variantは、引き続きFlow IRでは不正とする。外部C callbackの自動thunkとStateの自動global保存は、Flow IR variantの対応範囲とは別のABI・保存領域の拡張項目である。

本表を実装時の対応表とし、新しいFlow IR variantが増えたときは、対応表の更新とそのvariantのテストケース追加を同じ変更で行う。
