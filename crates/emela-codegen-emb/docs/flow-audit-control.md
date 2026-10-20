# Flow IR対応表：制御・Fiber

Flow IRの各要素と、C表現・実装箇所・既存試験の対応を示す。試験名は参照先であり、この表自体はテスト実行結果を示さない。

<!-- flow-audit:Stmt Let If Switch Loop Block Protect Dup Drop -->
<!-- flow-audit:Term Yield Return Fail Break Recur Defect Unwind Unreachable -->
<!-- flow-audit:FiberOp Scope Fork Deferred Join Await Interrupt Attempt Complete DeferredAwait Poll CauseKind CauseToString CauseRaise -->
<!-- flow-audit:OnFail Break Propagate Handle -->
<!-- flow-audit:SwitchOn Tag Lit -->
<!-- flow-audit:CaseKey Variant Lit -->
<!-- flow-audit:Stage Flow ClosuresConverted DefectsExplicit StateMachines RefCounted -->

## Stmt

| variant | 実装・所有権 | 既存試験の参照 | 状態・未検証 |
| --- | --- | --- | --- |
| `Stmt::Let` | 上記共通処理。managed destinationの旧値解放、borrowed projectorのretain、所有flagを扱う。FlowのlandingはNone | `emit::compiles_core_int_main_through_flow_ir_and_runs_generated_c`、`suspend::suspend_frames_own_string_inputs_and_return_owned_results_after_reuse`、`flow::rejects_heap_only_unwind_and_explicit_landing_edges_at_flow_stage` | 実装＋回帰。値ごとのdiscard監査は値/builtin資料を参照 |
| `Stmt::If` | 同期C分岐、suspend `Node::Branch`と各armの`Node::Assign`。managed arm値はdestinationへ保持 | `emit::compiles_if_statements_and_boolean_short_circuiting`、`fiber::aggregate::string_aggregates_own_borrowed_inputs_and_native_results_across_suspend_and_cancellation` | 実装＋回帰。全型での分岐直積は未検証 |
| `Stmt::Switch` | `emit_switch`、suspend `Node::Switch`。tag/Int32/Int64とdefaultを扱い、defaultなしtagの不正値はdefect | `emit::emits_and_runs_int32_and_int64_literal_switches`、`emit::emits_and_runs_exhaustive_payload_enum_switch_and_defects_on_invalid_tag`、`suspend::suspend_switch_handles_payload_and_nullary_tags` | 実装＋回帰。defaultなしliteralはFlow不正 |
| `Stmt::Loop` | `emit_loop`と`emit_recur`、suspend `LoopTarget` / `Node::Assign`。全引数を一度読む→retain→旧値release→更新の並列代入 | `emit::emits_and_runs_recur_with_multiple_loop_variables`、`suspend::suspend_parallel_recur_retains_managed_values_before_replacing_them`、`fiber::aggregate::parallel_recur_and_discarded_native_calls_reclaim_string_tuple_results` | 実装＋回帰。local全体のlast-use再利用は最適化候補 |
| `Stmt::Block` | 同期`BlockTarget`とlabel、suspend `Graph::blocks`。`Break`/`OnFail::Break`で値・errorを保持して脱出 | `emit::emits_and_runs_break_to_an_enclosing_block`、`suspend::suspend_loop_preserves_parallel_recur_and_block_result`、`fiber::payload::native_failure_channels_own_formatted_parse_errors_and_list_payloads` | 実装＋回帰。errorのAny descriptorと通常boxを同じものと扱わない |
| `Stmt::Protect` | `emit_protect` / `emit_pending_cleanups`、suspend `Node::Protect/Leave/CleanupEnd`と`runtime/unwind.c`。cleanupのerror slotを本体と分離 | `emit::runs_protect_cleanup_on_yield_break_and_return`、`fiber::protect::suspend_protect_runs_cleanup_on_yield_return_and_break_without_heap`、`fiber::failure::caught_failure_inside_suspending_release_does_not_replace_the_original_error`、`fiber::protect::cancellation_preserves_a_suspending_release_and_its_pending_task` | 実装＋回帰。defect即時停止はターゲット契約。cleanupの永久待ちを無条件に打ち切らない |
| `Stmt::Dup` | Flowでは明示RCを受け取らず、`verify_flow`→共有verifierで拒否 | `flow::rejects_heap_only_values_and_reference_count_statements_at_flow_stage` | Flow禁止。backendが必要なretainを挿入 |
| `Stmt::Drop` | 同上。共有verifierの`stage_at_least(RefCounted)`で拒否 |  | Flow禁止。backendが必要なreleaseを挿入 |

## Term

| variant | 実装・所有権 | 既存試験の参照 | 状態・未検証 |
| --- | --- | --- | --- |
| `Term::Yield` | `emit_compound_sequence`、suspend `Graph::assign`。値を保持してcompound destinationへ渡す。cleanupはvalueless Yield | `emit::emits_and_runs_a_block_that_yields_a_value`、`fiber::protect::suspend_protect_runs_cleanup_on_yield_return_and_break_without_heap` | 実装＋回帰。関数/closure本体の最終YieldはFlow不正 |
| `Term::Return` | `emit_return`、suspend `Node::Return`。caller storageへの移送/保持とlocal/frame破棄。途中のProtectを先に実行 | `emit::returns_dynamic_string_append_into_the_callers_buffer`、`fiber::aggregate_abi::scope_returns_string_enum_exits_after_success_or_interruption_and_frame_reuse`、`fiber::protect::suspend_cleanup_waits_for_an_external_task_before_publishing_the_result` | 実装＋回帰。Unit/NeverはC値なし。公開suspendのNULL出力も試験 |
| `Term::Fail` | `emit_error_assignment` / `emit_failure_return`、suspend `Node::SetError/Fail`。errorを所有しcleanup後に配送 | `emit::propagates_catches_and_reports_fallible_calls`、`fiber::failure::suspend_failures_cross_calls_and_preserve_cleanup_before_escape`、`fiber::payload::list_error_payloads_survive_suspending_cleanup_and_are_dropped_on_cancellation` | 実装＋回帰。fails=falseの伝播は拒否 |
| `Term::Break` | 同期labelとprotect depth、suspend `Graph::leave`。block結果の保持後に必要なcleanupを実行 | `emit::emits_and_runs_break_to_an_enclosing_block`、`fiber::protect::suspend_protect_runs_cleanup_on_yield_return_and_break_without_heap`、`fiber::function_aggregate::any_fields_nested_in_error_payloads_use_owned_boxes_and_indirect_failure_channels` | 実装＋回帰。cleanupを越えて外へbreakするFlowはverifierが拒否 |
| `Term::Recur` | `emit_recur`、suspend `Node::Assign`。innermost loop引数の並列置換 | `emit::emits_and_runs_recur_with_multiple_loop_variables`、`suspend::suspend_parallel_recur_retains_managed_values_before_replacing_them`、`flow::rejects_recur_crossing_protect_and_return_leaving_cleanup` | 実装＋回帰。Protectを越えるrecurは正常Flowでは禁止。Protect内のloopを回るrecurとは異なる |
| `Term::Defect` | `emit_defect` / suspend `Node::Defect`。kind、pointerとbyte長をboard hookへ渡す | `emit::sends_dynamic_defect_string_views_to_the_board_hook`、`emit::classifies_a_static_todo_defect`、`fiber::protect::defects_in_suspend_protect_keep_the_configured_immediate_stop_policy` | 実装＋回帰。cleanup・解放・Attempt捕捉を行わない既定契約 |
| `Term::Unwind` | 共有verifierのstage検査で拒否 | `flow::rejects_heap_only_unwind_and_explicit_landing_edges_at_flow_stage` | Flow禁止。保留defectを持つHeap IR専用 |
| `Term::Unreachable` | 同期/Graphとも到達時はASSERT defect。通常の非到達枝としても許可 | `emit::synchronous_exhaustive_matches_and_list_loops_accept_unreachable_terminators`、`fiber::attempt::attempt_keeps_the_embedded_policy_of_stopping_immediately_on_defect` | 実装＋回帰。任意の未初期化値を返して継続しない |

## FiberOp

| operation | 実装箇所 | 既存試験の参照 | 状態・所有権/境界 |
| --- | --- | --- | --- |
| `FiberOp::Scope` | `suspend::emit_function`のScope分岐、`runtime/scope.c` | `fiber::scope_starts_children_at_real_suspension_and_reclaims_waiting_tasks`、`fiber::failure::failing_and_infallible_scopes_share_the_fork_operation_abi`、`fiber::aggregate_abi::scope_returns_string_enum_exits_after_success_or_interruption_and_frame_reuse` | 実装＋回帰。fails混在、結果NULL、兄弟cleanup待ち、managed成功/失敗を確認 |
| `FiberOp::Fork` | `suspend::emit_scope_runtime` / `emit_fiber_value`、`emela_child_begin` | `fiber::child_reclaim::long_lived_scope_reclaims_completed_children_with_copied_handles_and_repeated_join_await`、`fiber::child_reclaim::discarded_completed_children_release_results_before_scope_end_and_survive_cancellation` | 実装＋回帰。実行/handle/waiterが所有しScope一覧はweak。未読のhandleをframeへ残さない |
| `FiberOp::Deferred` | `suspend::emit_fiber_value`、`runtime/deferred.c` | `deferred::discarded_deferreds_reuse_one_slot_within_a_long_lived_scope`、`deferred::deferred_pool_exhaustion_uses_the_defined_defect` | 実装＋回帰。handle寿命とslot generationを分ける |
| `FiberOp::Join` | `child::emit_wait` / `emit_runtime`、`runtime/child_wait.c` | `fiber::join::join_reuses_completed_results_without_yielding_and_waits_for_discarded_unit`、`fiber::join::join_results_own_strings_and_lists_after_scope_and_frame_reuse`、`fiber::interrupt::join_of_an_interrupted_child_is_the_defined_panic_defect` | 実装＋回帰。完了結果は反復読取可能。Unit/discardでも待機し、中断済み子のJoinはpanic |
| `FiberOp::Await` | `child::emit_wait` / `emit_runtime` | `fiber::await_::await_returns_success_and_can_mix_with_repeated_join_without_yielding`、`fiber::interrupt::await_of_interrupted_children_returns_owned_failures_repeatedly_and_for_unit` | 実装＋回帰。Exitの成功値/Causeを所有し、NULL結果は破棄。error/notesはScopeとも共有 |
| `FiberOp::Interrupt` | `child::emit_wait`、`runtime/child_wait.c` / `unwind.c` | `fiber::interrupt::interrupt_waits_for_suspending_cleanup_and_ignores_old_task_notifications`、`fiber::interrupt::self_interrupt_unwinds_the_body_and_is_deferred_inside_cleanup` | 実装＋回帰。cleanup完了まで待つ。hardwareのcancelそのものではない |
| `FiberOp::Attempt` | `cause::emit_attempts` / `emit_attempt_call`、同期/非同期callback | `fiber::attempt::attempt_captures_sync_failure_and_returns_owned_cause`、`fiber::attempt::attempt_owns_managed_success_and_releases_discarded_results`、`fiber::attempt::attempt_waits_for_async_cleanup_before_catching_failure` | 実装＋回帰。Fail捕捉・Interrupt非捕捉。Dieは既定即時停止契約により捕捉しない |
| `FiberOp::Complete` | `deferred::emit_complete` / `emit_runtime`、`emela_deferred_prepare_complete` | `deferred::complete_stores_once_and_scope_end_breaks_cycles_through_captured_deferreds`、`deferred::complete_copies_borrowed_string_and_poll_retains_it_past_scope_end` | 実装＋回帰。最初の値だけ保存。borrowed Stringをsnapshotし、複数回のcompleteで旧値を失わない |
| `FiberOp::DeferredAwait` | `deferred::emit_await`、`runtime/deferred_await.c` | `deferred::await_resumes_all_waiters_in_fifo_order_after_completer_finishes`、`deferred::await_preserves_string_and_list_results_past_frame_and_scope_reuse` | 実装＋回帰。NULL/Unitも待機し、配送前取消ではwaiterとresult所有を解放 |
| `FiberOp::Poll` | `deferred::emit_poll` / `emit_runtime` | `deferred::poll_observes_empty_and_first_value_for_scalar_and_unit_without_yielding`、`fiber::aggregate::deferred_complete_poll_and_await_share_owned_string_aggregates_past_scope_end` | 実装＋回帰。Optionを保持。dstなしは値をcopyせずhandle/Scope有効性を検査 |
| `FiberOp::CauseKind` | `cause::emit_kind` / `emela_cause_checked` | `fiber::cause::cause_kind_reads_failure_without_consuming_the_cause` | 実装＋回帰。観測のみでCauseを消費しない |
| `FiberOp::CauseToString` | `cause::emit_to_string`、`StringBufferPlan` | `fiber::cause::cause_to_string_uses_error_show_and_caller_output_buffer`、`fiber::notes::note_display_checks_the_callers_remaining_capacity` | 実装＋回帰。出力容量を検査。dstなしでも表示評価と容量defectを維持する |
| `FiberOp::CauseRaise` | `emit_statement_inner`のCauseRaise分岐、`cause::emit_infallible_raise` | `fiber::cause::cause_raise_preserves_the_error_and_runs_native_cleanup_before_escape`、`fiber::cause::raised_interruption_crosses_sync_calls_and_attempt_without_becoming_a_value`、`fiber::cause::failed_empty_cause_raised_in_cleanup_returns_to_its_failing_scope` | 実装＋回帰。Neverで後続を実行しない。Fail/Interruptを分離し、空集合Causeのsource正規化は`flow_from_core`内だけ |

## failure edge・landing・switch分類

| 項目 | 実装/検証 | 既存試験の参照 |
| --- | --- | --- |
| `OnFail::Break` | `emit_call_failure_edge`、`Graph::failure`。errorを所有してlabelのdestinationへ渡し、protect depthに応じcleanup | `emit::propagates_catches_and_reports_fallible_calls`、`fiber::failure::suspend_failures_cross_calls_and_preserve_cleanup_before_escape` |
| `OnFail::Propagate` | 上記と`emit_failure_return` / `Node::Fail`。fails=falseへの伝播は拒否 | `fiber::failure::uncaught_suspend_failure_stops_at_public_boundary_after_release`、`fiber::function_aggregate::failing_indirect_aggregate_calls_release_discarded_successes_and_preserve_string_errors` |
| `OnFail::Handle` | Heap IRの明示landing。Flowでは共有verifierが拒否 | `flow::rejects_heap_only_failure_handle_at_flow_stage` |
| `on_defect` / `on_suspend` | Flow入力では両方None。共有verifierのstage検査。backendのsuspend Graphは独自に作る | `flow::rejects_heap_only_unwind_and_explicit_landing_edges_at_flow_stage` |
| `SwitchOn::Tag` / `CaseKey::Variant` | enumのnullary/inline/recursive representationに応じtagを読む | `emit::emits_and_runs_switches_on_nullary_enum_representations`、`suspend::suspend_switch_handles_payload_and_nullary_tags` |
| `SwitchOn::Lit` / `CaseKey::Lit` | Int32/Int64の定数case、default必須 | `emit::emits_and_runs_int32_and_int64_literal_switches` |
| `Stage::Flow` | `compile_flow`が`verify_flow`を先に呼ぶ。共有verifierに加えてHeap runtimeと未lower builtinを拒否 | `flow::lowers_and_verifies_core_as_flow_ir` |
| `Stage::ClosuresConverted` / `Stage::DefectsExplicit` / `Stage::StateMachines` / `Stage::RefCounted` | stageそのものを入口で拒否し、Heap passを呼ばない | `flow::rejects_every_heap_stage_before_codegen` |
