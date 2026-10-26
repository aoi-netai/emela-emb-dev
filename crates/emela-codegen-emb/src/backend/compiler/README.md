# 共通コンパイラー

[compile_flowとCompiledModule](mod.rs) がFlow IRを検証・変換し、接続先に依存しない中間生成物を返す。closure持ち上げ、関数coerce・handler・scope・constの準備結果を保持する。中間生成物の利用方法と制約は [全体の説明](../README.md) を参照。

接続先を選んだ後、[emit.rs](emit.rs) の `emit` が配置・所有権・scheduler等を計画し、[features](features) の専用入口でCを生成する。一般的な外部関数の検証・所有権処理は [foundation/external.rs](foundation/external.rs)、C名やTypeの判断は各featureの担当に残す。

各機能は生成処理と同じfileに、必要に応じて次の計画用入口を持つ。型と戻り値の契約は [foundation/analysis.rs](foundation/analysis.rs) にある。

- `string_behavior`: 出力bufferの必要条件、入力の参照が操作後にも残るか、入力bufferの再利用候補。
- `string_bound`: 入力の既知の長さから求める出力長の上限。反復計算中に再評価する。
- `result_ownership`: 結果の所有権を受け取れるか、retainによる取得が必要か。
- `needs_heap` / `is_known_empty`: 操作固有のheap利用条件と空Listの情報。
- `require_runtime`: 必要なC helperとplatform hook。

解析のdispatcherはenumのvariantを網羅的に分岐し、処理不要なvariantも明示する。enum全体を受ける分岐にwildcardや暗黙のfallbackを置かず、新variant追加時には各解析の判断が揃うまでコンパイルエラーにする。この制約は、variantの引数やoperandの内容を調べる局所的なpattern matchには適用しない。

[string_buffers.rs](foundation/string_buffers.rs) はこれらの情報を使い、使用回数・最終使用位置・長さの反復計算・bufferのslot割当・呼び出し先からの出力ABIの伝播を行う。[heap_plan.rs](foundation/heap_plan.rs) は操作の要求を型・失敗channel・closure・戻り値の情報と集約する。これらの共有計画処理はBuiltinやFiberOpを直接分岐しない。

[runtime_plan.rs](foundation/runtime_plan.rs) は機能からの要求を集約し、helperの依存関係を解決して一度ずつ生成する。生成順は既存出力と同じ順に固定している。操作固有のC helperは担当feature、複数の機能から必要になる文字列・整数のhelperは [string_runtime.rs](foundation/string_runtime.rs) / [numeric_runtime.rs](foundation/numeric_runtime.rs) に置く。derived操作や外部ABIなどから生じる要求も合流させる。

例えば [StringAppend](features/builtins/string_append.rs) は `AppendMode` で定数連結・入力参照・動的連結を分類し、式生成・buffer計画・長さ計算・所有権・runtime要求に同じ判断を使う。実際のbuffer再利用は共有計画側が使用回数や生存区間を調べて確定する。

具体的なI2C・SPIやボード名の認識は行わない。[Target](target.rs) が次の情報・処理を供給する。

- 外部関数の接続先と、接続側が実装するadapterの識別
- 接続用header、adapter・補助Cコードの生成
- 計算済みの型配置・呼び出し関係・heap利用情報を使う検査
- 起動関数の宣言と本体

`Target`を実装する接続側は、共通コンパイラーの入口を再実行しない。[LinkPreparation](foundation/link.rs) で中間生成物のcopyへ制限した変更を加える。`CompiledModule`のIRとside tableは非公開で、読み取り用accessorだけを持つ。追加の到達可能rootは `finish(roots)` で検査し、最終C生成へ渡せる `LinkedModule` を作る。共通側は接続先の具体的なmoduleをimportしない。

許可する変更と禁止する変更は [全体の説明](../README.md) を参照。接続側へ `&mut Module` を渡すAPIは設けない。既存関数の変更は、callback引数とその呼び出し引数をまとめて除去する共通の処理に限定する。追加関数も、export・const・新たなclosure環境を要求するものは受け付けない。共有verifierでの検査時だけcaptureを暗黙の定義として補い、実際のABIへ引数を追加しない。

[platform.rs](platform.rs) は共通runtimeのhook宣言を生成する。既存出力の比較とruntime ABIの互換性のため、C上の名前には `emela_board_*` を残している。hookの実装は実行環境側が供給する。
