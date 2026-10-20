# Flow IR対応表：値・型・所有権

Flow IRの各要素と、C表現・実装箇所・既存試験の対応を示す。試験名は参照先であり、この表自体はテスト実行結果を示さない。

<!-- flow-audit:Value Const Prim Call Builtin Make Field Tag Tuple TupleGet List ListIsEmpty ListHead ListTail FnRef Closure MakeClosure EnvGet EvLookup EvExtend Handler Fiber IsError AsError Runtime Coerce -->
<!-- flow-audit:Const Int Int64 Float Bool String Unit -->
<!-- flow-audit:PrimOp Add Sub Mul Div Rem Neg Not Eq Ne Lt Le Gt Ge -->
<!-- flow-audit:PrimTy I32 I64 F64 String -->
<!-- flow-audit:Callee Direct Indirect Op -->
<!-- flow-audit:Operand Var Const -->

## Value全variant

| 行 | 実装箇所と現状 | 既存試験の参照 |
| --- | --- | --- |
| `Value::Const` | `emit.rs::constant_expression`。6種をC literal/viewへ出す。Unitは変数を持たない。 | `emits_c_for_a_function_returning_an_int_constant`、`compiles_core_bool_and_unit_constants`、`emits_and_runs_int64_constants_at_the_signed_boundaries`、`emits_and_runs_float_constants_including_non_finite_values`、`emits_utf8_and_embedded_nul_string_constants_as_views` |
| `Value::Prim` | `primitive_expression`と型別helper。整数wrapと除算defect、IEEE Float、String内容比較。 | `emits_and_runs_int32_primitive_operations`、`emits_and_runs_int64_primitive_operations_with_wrapping_semantics`、`emits_and_runs_float_primitive_operations`、`string_comparison::source_comparisons_are_heapless_and_allow_isr_and_constant_space_tail_loops` |
| `Value::Call` | `emit_statement_inner`のDirect、`emit_indirect_call`、`emit_handler_operation_call`、`suspend.rs`。結果の所有権取得と捨てた結果のreleaseを行う。 | `compiles_direct_calls_to_internal_and_external_c_functions`、`propagates_catches_and_reports_fallible_calls`、`passes_any_values_through_function_value_parameters_and_results`、`suspend::suspend_closure_keeps_captures_after_the_original_function_value_is_released` |
| `Value::Builtin` | 詳細はBuiltin担当。 | `emit_builtin`、`emit_cell_builtin` |
| `Value::Make` | `emit_make`、`emit_aggregate_field_store`、`TypeLayouts::collect`。inline enumと再帰する辺の箱、参照保持。 | `constructs_payload_and_unit_enum_variants_as_inline_values`、`boxes_recursive_enum_fields_and_releases_deep_values_iteratively`、`boxes_recursive_fields_nested_inside_tuples` |
| `Value::Field` | `emit_field`。recursive boxを参照解除し、managed結果を保持する。 | `reads_generic_enum_fields_from_inline_values`、`stores_and_releases_list_references_inside_enum_fields`、`owned::recursive_heap_payload_promotion_and_destruction_do_not_follow_value_depth_on_stack` |
| `Value::Tag` | `emit_tag`。nullary enumは整数、payload enumはtag field。 | `reads_tags_from_payload_and_nullary_enums`、`emits_and_runs_switches_on_nullary_enum_representations` |
| `Value::Tuple` | `emit_tuple`。Unit位置を保ったinline structと、recursive edgeの箱。 | `constructs_inline_tuples_and_keeps_unit_field_positions`、`stores_and_releases_list_references_inside_tuples_and_list_elements`、`owned::heap_payload_promotion_owns_inline_strings_and_shares_existing_backing` |
| `Value::TupleGet` | `emit_tuple_get`。inline fieldまたはrecursive boxから取得しmanaged結果を保持。 | `reads_tuple_fields_and_ignores_unit_projections`、`stores_and_releases_list_references_inside_tuples_and_list_elements` |
| `Value::List` | `emit_list_value`とtyped node。tail・headを保持し反復destroyで解放。 | `allocates_and_releases_lists_in_the_fixed_heap`、`reads_list_heads_and_keeps_shared_tails_alive`、`stores_and_releases_nested_lists`、`owned::returned_list_owns_formatted_strings_after_the_producer_stack_returns` |
| `Value::ListIsEmpty` | NULL比較。 | `emits_empty_list_and_checks_list_emptiness`、`discarded_list_emptiness_is_heapless` |
| `Value::ListHead` | `emit_list_head`。非空条件は入力の制御フローの前提。 | `reads_list_heads_and_keeps_shared_tails_alive` |
| `Value::ListTail` | `emit_list_tail`。使う結果は独立retain、捨てる結果は参照追加なし。 | `reads_list_heads_and_keeps_shared_tails_alive`、`discarded_list_tail_preserves_the_source_owner` |
| `Value::FnRef` | `validate_function_refs`、`function_ref_adapter`、lifted closureの環境構築。 | `adapts_a_function_value_to_effect_evidence_and_failure_call_abis`、`suspend::suspend_fnrefs_dispatch_to_external_tasks_and_internal_frames` |
| `Value::Closure` | `static_closure.rs`でbackend内lift。静的captureまたはtyped heap環境。 | `captures_runtime_values_in_a_fixed_heap_closure_environment`、`lifts_nested_closures_that_capture_outer_closure_values`、`cell::cycles_through_captured_cells_preserve_live_callbacks_and_reclaim_the_last_root` |
| `Value::MakeClosure` | Flow対象外。 | `compile_flow` |
| `Value::EnvGet` | Flow対象外。 |  |
| `Value::EvLookup` | `emit_evidence_lookup`。後から追加したbindingを優先し、欠落はdefect。 | `dynamically_dispatches_through_a_fixed_heap_evidence_table`、`passes_dynamic_effect_evidence_to_a_captured_closure` |
| `Value::EvExtend` | `emit_evidence_extend`。nodeがhandlerとparentを所有、空bindingはalias。 | `dynamically_dispatches_through_a_fixed_heap_evidence_table`、`passes_dynamic_effect_evidence_to_a_captured_closure` |
| `Value::Handler` | `emit_handler_value`。active flagと各operation closureを保持。 | `passes_dynamic_effect_evidence_to_a_captured_closure`、`suspend::duplicate_resume_reports_the_operation_and_handler` |
| `Value::Fiber` | 詳細は制御・Fiber担当。 | `fiber` |
| `Value::IsError` | Any/EmelaErrorの正tagをenum IDと比較。 | `propagates_catches_and_reports_fallible_calls` |
| `Value::AsError` | matched error payloadからtyped enumを読む。managed結果はretain。 | `cell::cell_references_in_native_error_payloads_outlive_the_failing_frame` |
| `Value::Runtime` | 詳細はRuntimeOp担当。 | `emit_runtime` |
| `Value::Coerce` | `emit_coerce`、`function_coerce.rs`、`owned.rs`。Fn ABI wrapper、typed Any box、runtime handle、errorAny所有。 | `adapts_a_function_value_to_effect_evidence_and_failure_call_abis`、`passes_any_values_through_function_value_parameters_and_results`、`any_data::tagged_error_any_values_remain_normal_data_through_child_and_deferred_lifetimes`、`suspend::coerce_adapts_sync_and_suspend_function_values_to_a_suspend_evidence_abi` |

## Primitiveの全組合せ

| `PrimOp` | `PrimTy::I32` | `PrimTy::I64` | `PrimTy::F64` | `PrimTy::String` |
| --- | --- | --- | --- | --- |
| `Add` | wrap、既存I32試験 | wrap、既存I64試験 | double加算、既存Float試験 | verifier拒否 |
| `Sub` | wrap、既存I32試験 | wrap、既存I64試験 | double減算、既存Float試験 | verifier拒否 |
| `Mul` | wrap、既存I32試験 | wrap、既存I64試験 | double乗算、既存Float試験 | verifier拒否 |
| `Div` | 0 defect・MIN/-1、専用試験 | 同左 | IEEE除算、既存Float試験 | verifier拒否 |
| `Rem` | 0 defect・MIN%-1、専用試験 | 同左 | fmod、既存Float試験 | verifier拒否 |
| `Neg` | MIN wrap、既存I32試験 | MIN wrap、既存I64試験 | double符号、既存Float試験 | verifier拒否 |
| `Not` | 0/非0から1/0、既存I32試験 | verifier拒否 | verifier拒否 | verifier拒否 |
| `Eq` | 既存I32試験 | 既存I64試験 | NaNを含む既存Float試験 | String比較専用4試験 |
| `Ne` | 既存I32試験 | 既存I64試験 | 同上 | 同上 |
| `Lt` | 既存I32試験 | 既存I64試験 | 同上 | 同上 |
| `Le` | 既存I32試験 | 既存I64試験 | 同上 | 同上 |
| `Gt` | 既存I32試験 | 既存I64試験 | 同上 | 同上 |
| `Ge` | 既存I32試験 | 既存I64試験 | 同上 | 同上 |

## ConstとCallee

| Const variant | 出力と試験 |
| --- | --- |
| `Const::Int` | INT32 literal。`emits_c_for_a_function_returning_an_int_constant`。 |
| `Const::Int64` | INT64 literal。`emits_and_runs_int64_constants_at_the_signed_boundaries`。 |
| `Const::Float` | double / NaN / Infinity。`emits_and_runs_float_constants_including_non_finite_values`。 |
| `Const::Bool` | 0/1。`compiles_core_bool_and_unit_constants`。 |
| `Const::String` | pointer/uint16 byte長、読み取り専用領域、owner NULL。`emits_utf8_and_embedded_nul_string_constants_as_views`。 |
| `Const::Unit` | C値・変数なし。`compiles_core_bool_and_unit_constants`、tuple/enum/ListのUnit位置試験。 |

| Callee variant | 実装・試験 |
| --- | --- |
| `Callee::Direct` | targetのret/fails/suspendsからABIを決める。`compiles_direct_calls_to_internal_and_external_c_functions`、`propagates_catches_and_reports_fallible_calls`、`suspend::suspend_recursion_resumes_into_caller_storage_and_reuses_frames`。 |
| `Callee::Indirect` | typed function recordのcall/env。`passes_any_values_through_function_value_parameters_and_results`、`propagates_failure_from_a_captured_closure_through_its_indirect_call`、`suspend::suspend_closure_keeps_captures_after_the_original_function_value_is_released`。 |
| `Callee::Op` | handlerのactive flag・operation pointerを確認。同期はstatic解決できる場合にIndirectへ、動的/suspendはruntimeへ。`passes_dynamic_effect_evidence_to_a_captured_closure`、`suspend::suspend_handler_source_waits_and_resumes_into_the_callers_result`。 |

## Operand

| variant | 実装・根拠 |
| --- | --- |
| `Operand::Var` | `operand_expression`が関数内VarIdをC名へ対応付け、型は`Function.vars`から得る。`emits_and_runs_recur_with_multiple_loop_variables`とclosure/aggregate所有権試験が実際の値移送を確認する。 |
| `Operand::Const` | `constant_expression`。6種のConst試験およびprimitiveのliteral operandで確認。Unitはcontainer等の位置保持用であり通常のC値を持たない。 |

## 入力と資源の境界

入力は検証済みの`Stage::Flow`とし、Heap IR専用の値・操作は拒否する。固定heap・pool・Stringの容量超過はdefectとして扱う。
