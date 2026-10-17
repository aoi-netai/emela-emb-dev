//! GPIO を含む直線的な Core IR を C11 に変換する。

use std::collections::{BTreeSet, VecDeque};
use std::fmt::Write as _;

use emela_core::{Callee, Expr, Function, Lit, Module, Pat, Type, TypeId, VariantFields};

use crate::{Error, Output};

pub(super) fn compile(module: &Module) -> Result<Output, Vec<Error>> {
    let Some((main_id, _)) = module
        .functions
        .iter()
        .find(|(_, function)| function.exported && function.name == "main")
    else {
        return Err(vec![error("組み込みC出力には `main` 関数が必要です")]);
    };
    if module.functions[main_id].params.iter().any(|&local| {
        !matches!(module.ty(module.locals[local].ty), Type::Unit | Type::Evidence)
    }) {
        return Err(vec![error("組み込みCの `main` 関数は引数を取れません")]);
    }

    let functions = reachable_functions(module, main_id)?;
    let map_helpers = option_map_helpers(module, &functions)?;
    let option_types = option_types(module, &map_helpers);
    check_interrupt_handlers(module, &functions)?;
    let mut emitter = Emitter {
        module,
        functions: &functions,
        map_helpers: &map_helpers,
        out: String::new(),
        errors: Vec::new(),
        temporary: 0,
        board_handlers: Vec::new(),
    };
    emitter.write_header();
    emitter.write_prototypes();
    emitter.write_map_helper_prototypes();
    if !emitter.errors.is_empty() {
        return Err(emitter.errors);
    }
    emitter.write_map_helpers();
    for &id in &functions {
        emitter.write_function(id);
    }
    emitter.out.push_str("\nint main(void) {\n  ");
    let c_main = function_name(main_id, &module.functions[main_id]);
    writeln!(emitter.out, "{c_main}();\n  for (;;) {{}}\n}}").unwrap();
    if !emitter.errors.is_empty() {
        return Err(emitter.errors);
    }

    let gpio_header = gpio_header(module, &option_types).map_err(|message| vec![error(message)])?;
    let app_header = app_header(module, &functions, &emitter)?;
    Ok(Output {
        main: emitter.out,
        gpio_header,
        app_header,
    })
}

fn reachable_functions(module: &Module, main: emela_core::FnId) -> Result<Vec<emela_core::FnId>, Vec<Error>> {
    let mut reached = Vec::new();
    let mut queued = VecDeque::from([main]);
    queued.extend(
        module
            .functions
            .iter()
            .filter_map(|(id, function)| (function.exported && id != main).then_some(id)),
    );
    while let Some(id) = queued.pop_front() {
        if reached.contains(&id) {
            continue;
        }
        let function = &module.functions[id];
        if function.external.is_some() {
            return Err(vec![error(format!(
                "組み込みCの初期実装では外部関数 `{}` を出力できません",
                function.name
            ))]);
        }
        reached.push(id);
        let mut direct = Vec::new();
        collect_direct_calls(module, &function.body, &mut direct);
        queued.extend(direct);
    }
    Ok(reached)
}

fn collect_direct_calls(module: &Module, expr: &Expr, found: &mut Vec<emela_core::FnId>) {
    if let Expr::Call {
        callee: Callee::Direct { f, .. },
        args,
        ..
    } = expr
    {
        if is_option_map(module, *f) {
            if let Some(Expr::Fn { f: callback, .. }) = args.get(1) {
                found.push(*callback);
            }
        } else {
            found.push(*f);
        }
    }
    expr.for_each_child(&mut |child| collect_direct_calls(module, child, found));
}

fn is_option_map(module: &Module, function: emela_core::FnId) -> bool {
    let name = &module.functions[function].name;
    let name = name.split('[').next().unwrap_or(name);
    name == "Option.map" || name.ends_with(".Option.map")
}

fn option_map_helpers(
    module: &Module,
    functions: &[emela_core::FnId],
) -> Result<Vec<OptionMapHelper>, Vec<Error>> {
    let mut helpers = Vec::new();
    let mut errors = Vec::new();
    for &id in functions {
        let mut visit = |expr: &Expr| {
            if let Expr::Call {
                callee: Callee::Direct { f, .. },
                args,
                ..
            } = expr
            {
                if !is_option_map(module, *f) {
                    return;
                }
                let Some(Expr::Fn { f: callback, .. }) = args.get(1) else {
                    errors.push(error(
                        "組み込みCの `Option.map` では静的に分かる名前付き関数を渡してください",
                    ));
                    return;
                };
                let map = &module.functions[*f];
                if map.type_params != 0 || args.len() != 2 || map.params.len() < 2 {
                    errors.push(error("組み込みCの `Option.map` 呼び出し形式が不正です"));
                    return;
                }
                let input = module.locals[map.params[0]].ty;
                let output = map.ret;
                let (Some(input_payload), Some(output_payload)) = (
                    option_payload(module, input),
                    option_payload(module, output),
                ) else {
                    errors.push(error(
                        "組み込みCの `Option.map` は `Option[A] -> Option[B]` の形が必要です",
                    ));
                    return;
                };
                let callback_fn = &module.functions[*callback];
                if callback_fn.params.len() != 1
                    || module.locals[callback_fn.params[0]].ty != input_payload
                    || callback_fn.ret != output_payload
                    || callback_fn.type_params != 0
                    || callback_fn.fails
                    || callback_fn.suspends
                {
                    errors.push(error(
                        "組み込みCの `Option.map` コールバックは対応する型の純粋な単相化関数が必要です",
                    ));
                    return;
                }
                let helper = OptionMapHelper {
                    input,
                    output,
                    callback: *callback,
                };
                if !helpers.contains(&helper) {
                    helpers.push(helper);
                }
            }
        };
        visit_exprs(&module.functions[id].body, &mut visit);
    }
    if errors.is_empty() {
        Ok(helpers)
    } else {
        Err(errors)
    }
}

fn visit_exprs(expr: &Expr, visit: &mut impl FnMut(&Expr)) {
    visit(expr);
    expr.for_each_child(&mut |child| visit_exprs(child, visit));
}

fn board_handler_aliases(
    module: &Module,
    expr: &Expr,
) -> Vec<(emela_core::Local, emela_core::EffectId)> {
    let mut aliases = Vec::new();
    visit_exprs(expr, &mut |candidate| {
        let Expr::Let { var, value, .. } = candidate else {
            return;
        };
        let effect = match value.as_ref() {
            Expr::Use { effect, .. } => Some(*effect),
            Expr::Var(source) => aliases
                .iter()
                .find_map(|(bound, effect)| (*bound == *source).then_some(*effect)),
            _ => None,
        };
        let Some(effect) = effect else {
            return;
        };
        if matches!(module.ty(module.locals[*var].ty), Type::Handler(bound) if *bound == effect) {
            aliases.push((*var, effect));
        }
    });
    aliases
}

fn option_payload(module: &Module, ty: TypeId) -> Option<TypeId> {
    match module.ty(ty) {
        Type::Enum(id, args)
            if args.len() == 1
                && (module.option == Some(*id) || is_named(&module.enums[*id].name, "Option")) =>
        {
            Some(args[0])
        }
        _ => None,
    }
}

fn same_scalar_enum(module: &Module, lhs: &Expr, rhs: &Expr) -> bool {
    let (Some(lhs), Some(rhs)) = (expression_type(module, lhs), expression_type(module, rhs)) else {
        return false;
    };
    if lhs != rhs {
        return false;
    }
    let Type::Enum(enum_id, _) = module.ty(lhs) else {
        return false;
    };
    let def = &module.enums[*enum_id];
    def.name.ends_with(".GpioPin")
        || def.name.ends_with(".PinState")
        || def.name.ends_with(".Pull")
        || def.name.ends_with(".GpioMode")
        || def.name.ends_with(".Edge")
        || def.variants.iter().all(|variant| variant.tys.is_empty())
}

fn expression_type(module: &Module, expr: &Expr) -> Option<TypeId> {
    match expr {
        Expr::Var(local) => Some(module.locals[*local].ty),
        Expr::Lit(Lit::Int(_)) => Some(TypeId::INT),
        Expr::Lit(Lit::Int64(_)) => Some(TypeId::INT64),
        Expr::Lit(Lit::Bool(_)) => Some(TypeId::BOOL),
        Expr::Ctor { ctor, ty_args, .. } => module
            .types
            .get(&Type::Enum(ctor.enum_id, ty_args.clone())),
        Expr::Field { ctor, index, .. } => module.enums[ctor.enum_id].variants[ctor.variant]
            .tys
            .get(*index)
            .copied(),
        Expr::Call { callee, .. } => match callee {
            Callee::Direct { f, .. } => Some(module.functions[*f].ret),
            Callee::Op { effect, op, .. } => module.effects[*effect].ops.get(*op).map(|op| op.ret),
            Callee::Indirect(_) => None,
        },
        Expr::If { then, .. } => expression_type(module, then),
        _ => None,
    }
}

fn option_types(module: &Module, extra: &[OptionMapHelper]) -> Vec<TypeId> {
    fn visit(module: &Module, ty: TypeId, out: &mut BTreeSet<TypeId>) {
        match module.ty(ty) {
            Type::Tuple(items) => items.iter().for_each(|&item| visit(module, item, out)),
            Type::List(item) | Type::Cell(item) => visit(module, *item, out),
            Type::Enum(_, args) => {
                if option_payload(module, ty).is_some() {
                    out.insert(ty);
                }
                args.iter().for_each(|&item| visit(module, item, out));
            }
            Type::Fn(function) => {
                function.params.iter().for_each(|&item| visit(module, item, out));
                visit(module, function.ret, out);
            }
            _ => {}
        }
    }

    let mut out = BTreeSet::new();
    for (_, local) in module.locals.iter() {
        visit(module, local.ty, &mut out);
    }
    for (_, function) in module.functions.iter() {
        visit(module, function.ret, &mut out);
    }
    for helper in extra {
        visit(module, helper.input, &mut out);
        visit(module, helper.output, &mut out);
    }
    out.into_iter().collect()
}

fn check_interrupt_handlers(module: &Module, functions: &[emela_core::FnId]) -> Result<(), Vec<Error>> {
    let mut errors = Vec::new();
    let handlers: Vec<_> = functions
        .iter()
        .copied()
        .filter(|&id| {
            let function = &module.functions[id];
            if !function.exported || function.name == "main" {
                return false;
            }
            let event_params = function
                .params
                .iter()
                .filter(|&&local| is_gpio_event_type(module, module.locals[local].ty))
                .count();
            if event_params == 0 {
                return false;
            }
            let value_params = function
                .params
                .iter()
                .filter(|&&local| !matches!(module.ty(module.locals[local].ty), Type::Evidence | Type::Unit))
                .count();
            if event_params != 1
                || value_params != 1
                || !matches!(module.ty(function.ret), Type::Unit)
            {
                errors.push(error(format!(
                    "組み込みCの割り込み入口 `{}` は `fn(GpioEvent) -> ()` の形にしてください",
                    function.name
                )));
            }
            true
        })
        .collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut visited = Vec::new();
    let mut pending = VecDeque::from(handlers);
    while let Some(id) = pending.pop_front() {
        if visited.contains(&id) {
            continue;
        }
        visited.push(id);
        let function = &module.functions[id];
        if function.fails || function.suspends || function.external.is_some() {
            errors.push(error(format!(
                "割り込み関数 `{}` とその呼び出し先は失敗・中断・外部関数を使えません",
                function.name
            )));
            continue;
        }
        inspect_isr_expr(module, id, &function.body, &mut pending, &mut errors);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn is_gpio_event_type(module: &Module, ty: TypeId) -> bool {
    matches!(module.ty(ty), Type::Enum(enum_id, _) if module.enums[*enum_id].name.ends_with(".GpioEvent"))
}

fn inspect_isr_expr(
    module: &Module,
    owner: emela_core::FnId,
    expr: &Expr,
    pending: &mut VecDeque<emela_core::FnId>,
    errors: &mut Vec<Error>,
) {
    match expr {
        Expr::Call { callee, args, .. } => match callee {
            Callee::Direct { f, .. } if is_option_map(module, *f) => {
                if let Some(Expr::Fn { f, .. }) = args.get(1) {
                    pending.push_back(*f);
                } else {
                    errors.push(error(format!(
                        "割り込み関数 `{}` から呼ぶ `Option.map` の関数は静的に特定できません",
                        module.functions[owner].name
                    )));
                }
            }
            Callee::Direct { f, .. } => pending.push_back(*f),
            Callee::Op { effect, op, .. } => {
                let effect = &module.effects[*effect];
                let operation = effect.ops.get(*op).map(|op| op.name.as_str()).unwrap_or("");
                if effect.name != "Std.Embedded.Gpio.GPIO"
                    || !matches!(operation, "read" | "write" | "toggle")
                {
                    errors.push(error(format!(
                        "割り込み関数 `{}` では `{}` effect の `{}` 操作を使えません",
                        module.functions[owner].name, effect.name, operation
                    )));
                }
            }
            Callee::Indirect(_) => errors.push(error(format!(
                "割り込み関数 `{}` から間接関数呼び出しはできません",
                module.functions[owner].name
            ))),
        },
        Expr::Builtin { .. }
        | Expr::List { .. }
        | Expr::Lambda { .. }
        | Expr::Fiber { .. }
        | Expr::With { .. }
        | Expr::Provide { .. }
        | Expr::Handler { .. }
        | Expr::Loop { .. }
        | Expr::Recur(_) => errors.push(error(format!(
            "割り込み関数 `{}` からヒープまたは実行時制御を伴う式は使えません",
            module.functions[owner].name
        ))),
        _ => {}
    }
    expr.for_each_child(&mut |child| inspect_isr_expr(module, owner, child, pending, errors));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OptionMapHelper {
    input: TypeId,
    output: TypeId,
    callback: emela_core::FnId,
}

impl OptionMapHelper {
    fn name(self) -> String {
        format!(
            "emela_option_map_{}_{}_{}",
            u32::from(self.callback.into_raw()),
            self.input.index(),
            self.output.index()
        )
    }
}

struct Emitter<'a> {
    module: &'a Module,
    functions: &'a [emela_core::FnId],
    map_helpers: &'a [OptionMapHelper],
    out: String,
    errors: Vec<Error>,
    temporary: usize,
    /// `use GPIO` に束縛されたローカル。Cでは値を持たずボード境界へ直接変換する。
    board_handlers: Vec<(emela_core::Local, emela_core::EffectId)>,
}

impl Emitter<'_> {
    fn write_header(&mut self) {
        self.out.push_str(
            "#include <stdbool.h>\n#include <stdint.h>\n#include \"emela_gpio.h\"\n\n\
             static inline int32_t emela_i32_add(int32_t a, int32_t b) { return (int32_t)((uint32_t)a + (uint32_t)b); }\n\
             static inline int32_t emela_i32_sub(int32_t a, int32_t b) { return (int32_t)((uint32_t)a - (uint32_t)b); }\n\
             static inline int32_t emela_i32_mul(int32_t a, int32_t b) { return (int32_t)((uint32_t)a * (uint32_t)b); }\n\
             static inline int32_t emela_i32_neg(int32_t a) { return (int32_t)(0u - (uint32_t)a); }\n\
             static inline int64_t emela_i64_add(int64_t a, int64_t b) { return (int64_t)((uint64_t)a + (uint64_t)b); }\n\
             static inline int64_t emela_i64_sub(int64_t a, int64_t b) { return (int64_t)((uint64_t)a - (uint64_t)b); }\n\
             static inline int64_t emela_i64_mul(int64_t a, int64_t b) { return (int64_t)((uint64_t)a * (uint64_t)b); }\n\
             static inline int64_t emela_i64_neg(int64_t a) { return (int64_t)(0u - (uint64_t)a); }\n\n",
        );
    }

    fn write_prototypes(&mut self) {
        for &id in self.functions {
            let function = self.module.functions[id].clone();
            match self.function_signature(id, &function) {
                Ok(signature) => {
                    writeln!(self.out, "{signature};").unwrap();
                }
                Err(message) => self.errors.push(error(message)),
            }
        }
        self.out.push('\n');
    }

    fn write_map_helper_prototypes(&mut self) {
        for helper in self.map_helpers {
            let input = match self.c_type(helper.input) {
                Ok(ty) => ty,
                Err(message) => {
                    self.errors.push(error(message));
                    continue;
                }
            };
            let output = match self.c_type(helper.output) {
                Ok(ty) => ty,
                Err(message) => {
                    self.errors.push(error(message));
                    continue;
                }
            };
            let name = helper.name();
            writeln!(self.out, "static {output} {name}({input} value);").unwrap();
        }
        if !self.map_helpers.is_empty() {
            self.out.push('\n');
        }
    }

    fn write_map_helpers(&mut self) {
        for helper in self.map_helpers {
            let input = match self.c_type(helper.input) {
                Ok(ty) => ty,
                Err(message) => {
                    self.errors.push(error(message));
                    continue;
                }
            };
            let output = match self.c_type(helper.output) {
                Ok(ty) => ty,
                Err(message) => {
                    self.errors.push(error(message));
                    continue;
                }
            };
            let Some(input_payload) = option_payload(self.module, helper.input) else {
                self.errors.push(error("Option.map の入力型が Option ではありません"));
                continue;
            };
            let Some(output_payload) = option_payload(self.module, helper.output) else {
                self.errors.push(error("Option.map の出力型が Option ではありません"));
                continue;
            };
            let input_value = match self.c_type(input_payload) {
                Ok(ty) => ty,
                Err(message) => {
                    self.errors.push(error(message));
                    continue;
                }
            };
            let output_value = match self.c_type(output_payload) {
                Ok(ty) => ty,
                Err(message) => {
                    self.errors.push(error(message));
                    continue;
                }
            };
            if input_value == "void" || output_value == "void" {
                self.errors.push(error("組み込みCの `Option.map` では `()` 型を扱えません"));
                continue;
            }
            let name = helper.name();
            let callback = function_name(helper.callback, &self.module.functions[helper.callback]);
            writeln!(
                self.out,
                "static {output} {name}({input} value) {{\n  {output} result = {{ .is_some = false }};\n  if (value.is_some) {{\n    result.is_some = true;\n    result.value = {callback}(({input_value})value.value);\n  }}\n  return result;\n}}\n"
            )
            .unwrap();
        }
    }

    fn function_signature(
        &self,
        id: emela_core::FnId,
        function: &Function,
    ) -> Result<String, String> {
        if function.type_params != 0 {
            return Err(format!(
                "組み込みCの初期実装では多相関数 `{}` を出力できません",
                function.name
            ));
        }
        if function.fails || function.suspends {
            return Err(format!(
                "組み込みCの初期実装では失敗または中断する関数 `{}` を出力できません",
                function.name
            ));
        }
        let ret = self.c_type(function.ret)?;
        let params = function
            .params
            .iter()
            .filter_map(|&local| {
                let ty = self.module.locals[local].ty;
                (!matches!(self.module.ty(ty), Type::Unit | Type::Evidence)).then_some((local, ty))
            })
            .map(|(local, ty)| Ok(format!("{} {}", self.c_type(ty)?, local_name(local))))
            .collect::<Result<Vec<_>, String>>()?;
        let params = if params.is_empty() {
            "void".to_owned()
        } else {
            params.join(", ")
        };
        let linkage = if function.exported && function.name != "main" {
            ""
        } else {
            "static "
        };
        Ok(format!(
            "{linkage}{ret} {}({params})",
            function_name(id, function)
        ))
    }

    fn write_function(&mut self, id: emela_core::FnId) {
        let function = self.module.functions[id].clone();
        self.board_handlers = board_handler_aliases(self.module, &function.body);
        let signature = match self.function_signature(id, &function) {
            Ok(signature) => signature,
            Err(message) => {
                self.errors.push(error(message));
                return;
            }
        };
        writeln!(self.out, "{signature} {{").unwrap();
        self.write_tail(&function.body, function.ret, 1);
        self.out.push_str("}\n\n");
    }

    fn write_tail(&mut self, expr: &Expr, ret: TypeId, indent: usize) {
        match expr {
            Expr::Let { var, value, body } => {
                self.write_binding(*var, value, indent);
                self.write_tail(body, ret, indent);
            }
            Expr::If { cond, then, else_ } => {
                let cond = self.value(cond);
                match cond {
                    Ok(cond) => {
                        self.line(indent, &format!("if ({cond}) {{"));
                        self.write_tail(then, ret, indent + 1);
                        self.line(indent, "} else {");
                        self.write_tail(else_, ret, indent + 1);
                        self.line(indent, "}");
                    }
                    Err(message) => self.errors.push(error(message)),
                }
            }
            Expr::Match { scrutinee, arms } => self.write_option_match(scrutinee, arms, ret, indent),
            Expr::Call { .. } => match self.call(expr) {
                Ok((call, call_ret)) if matches!(self.module.ty(call_ret), Type::Unit) => {
                    self.line(indent, &format!("{call};"));
                    self.line(indent, "return;");
                }
                Ok((call, _)) => self.line(indent, &format!("return {call};")),
                Err(message) => self.errors.push(error(message)),
            },
            _ if matches!(self.module.ty(ret), Type::Unit) => {
                match self.value(expr) {
                    Ok(value) if value != "0" => self.line(indent, &format!("(void)({value});")),
                    Ok(_) => {}
                    Err(message) => self.errors.push(error(message)),
                }
                self.line(indent, "return;");
            }
            _ => match self.value(expr) {
                Ok(value) => self.line(indent, &format!("return {value};")),
                Err(message) => self.errors.push(error(message)),
            },
        }
    }

    fn write_binding(&mut self, local: emela_core::Local, value: &Expr, indent: usize) {
        let ty = self.module.locals[local].ty;
        if let Type::Handler(effect) = self.module.ty(ty) {
            if self
                .board_handlers
                .iter()
                .any(|(bound, bound_effect)| *bound == local && bound_effect == effect)
            {
                return;
            }
            self.errors
                .push(error("組み込みCではボード提供のeffect値だけを使えます"));
            return;
        }
        if matches!(self.module.ty(ty), Type::Unit | Type::Evidence) {
            if let Expr::Call { .. } = value {
                match self.call(value) {
                    Ok((call, _)) => self.line(indent, &format!("{call};")),
                    Err(message) => self.errors.push(error(message)),
                }
            }
            return;
        }
        let c_type = match self.c_type(ty) {
            Ok(c_type) => c_type,
            Err(message) => {
                self.errors.push(error(message));
                return;
            }
        };
        let initializer = if let Expr::Call { .. } = value {
            match self.call(value) {
                Ok((call, _)) => call,
                Err(message) => {
                    self.errors.push(error(message));
                    return;
                }
            }
        } else {
            match self.value(value) {
                Ok(value) => value,
                Err(message) => {
                    self.errors.push(error(message));
                    return;
                }
            }
        };
        self.line(
            indent,
            &format!("{c_type} {} = {initializer};", local_name(local)),
        );
    }

    fn write_option_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[emela_core::Arm],
        ret: TypeId,
        indent: usize,
    ) {
        let temp = format!("emela_match_{}", self.temporary);
        self.temporary += 1;
        let (value, option_ty) = match scrutinee {
            Expr::Call { .. } => match self.call(scrutinee) {
                Ok((call, ty)) => (call, ty),
                Err(message) => {
                    self.errors.push(error(message));
                    return;
                }
            },
            Expr::Var(local) => (local_name(*local), self.module.locals[*local].ty),
            _ => {
                self.errors.push(error("組み込みCの `match` は `Option` のローカル値または関数呼び出しを対象にしてください"));
                return;
            }
        };
        let Some(payload_ty) = option_payload(self.module, option_ty) else {
            self.errors.push(error("組み込みCの初期実装では `Option` 以外の `match` を出力できません"));
            return;
        };
        let option_c_ty = match self.c_type(option_ty) {
            Ok(ty) => ty,
            Err(message) => {
                self.errors.push(error(message));
                return;
            }
        };
        self.line(indent, &format!("{option_c_ty} {temp} = {value};"));

        let mut some = None;
        let mut none = None;
        for arm in arms {
            if arm.guard.is_some() {
                self.errors.push(error("組み込みCの `Option` 照合ではガード節を使えません"));
                return;
            }
            let Pat::Ctor { ctor, fields } = &arm.pat else {
                self.errors.push(error("組み込みCの `Option` 照合では Some と None のパターンが必要です"));
                return;
            };
            let def = &self.module.enums[ctor.enum_id];
            if !is_named(&def.name, "Option") || option_payload(self.module, option_ty).is_none() {
                self.errors.push(error("組み込みCの `match` は `Option` を対象にしてください"));
                return;
            }
            match ctor.variant {
                0 if fields.len() == 1 && some.is_none() => some = Some((arm, &fields[0])),
                1 if fields.is_empty() && none.is_none() => none = Some(arm),
                _ => {
                    self.errors.push(error("Option の Some / None パターンが不正です"));
                    return;
                }
            }
        }
        let (Some((some_arm, some_field)), Some(none_arm)) = (some, none) else {
            self.errors.push(error("組み込みCの `Option` 照合には Some と None の両方が必要です"));
            return;
        };

        self.line(indent, &format!("if ({temp}.is_some) {{"));
        match some_field {
            Pat::Bind(local) => {
                let payload_c_ty = match self.option_storage_type(payload_ty) {
                    Ok(ty) => ty,
                    Err(message) => {
                        self.errors.push(error(message));
                        return;
                    }
                };
                self.line(
                    indent + 1,
                    &format!("{payload_c_ty} {} = {temp}.value;", local_name(*local)),
                );
            }
            Pat::Wild => {}
            _ => {
                self.errors.push(error("Some の中では値を変数に束縛するか `_` を使ってください"));
                return;
            }
        }
        self.write_tail(&some_arm.body, ret, indent + 1);
        self.line(indent, "} else {");
        self.write_tail(&none_arm.body, ret, indent + 1);
        self.line(indent, "}");
    }

    fn call(&self, expr: &Expr) -> Result<(String, TypeId), String> {
        let Expr::Call {
            callee,
            args,
            fails,
            suspends,
        } = expr
        else {
            return Err("内部エラー: 関数呼び出しではありません".to_owned());
        };
        if *fails || *suspends {
            return Err("組み込みCの初期実装では失敗または中断する呼び出しを出力できません".to_owned());
        }
        match callee {
            Callee::Op {
                handler,
                effect,
                op,
                ..
            } => {
                let board_handler = match handler.as_ref() {
                    Expr::Use {
                        effect: handler_effect,
                        ..
                    } => handler_effect == effect,
                    Expr::Var(local) => self
                        .board_handlers
                        .iter()
                        .any(|(bound, handler_effect)| bound == local && handler_effect == effect),
                    _ => false,
                };
                if !board_handler {
                    return Err("組み込みGPIOではボード提供のハンドラーのみ使えます".to_owned());
                }
                let effect = &self.module.effects[*effect];
                if effect.name != "Std.Embedded.Gpio.GPIO" {
                    return Err(format!(
                        "組み込みCの初期実装で未対応のeffect `{}` が使われています",
                        effect.name
                    ));
                }
                let operation = effect.ops.get(*op).ok_or_else(|| {
                    "内部エラー: GPIO 操作番号が定義範囲外です".to_owned()
                })?;
                let c_name = match operation.name.as_str() {
                    "configure" | "read" | "write" | "toggle" | "enable_interrupt"
                    | "disable_interrupt" => {
                        format!("emela_gpio_{}", operation.name)
                    }
                    other => return Err(format!("GPIO 操作 `{other}` は未対応です")),
                };
                if args.len() != operation.params.len() {
                    return Err(format!(
                        "GPIO 操作 `{}` の引数個数が不正です",
                        operation.name
                    ));
                }
                let args = args
                    .iter()
                    .map(|arg| self.value(arg))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ");
                Ok((format!("{c_name}({args})"), operation.ret))
            }
            Callee::Direct { f, .. } => {
                let function = &self.module.functions[*f];
                if is_option_map(self.module, *f) {
                    let Some(Expr::Fn { f: callback, .. }) = args.get(1) else {
                        return Err("組み込みCの `Option.map` では静的に分かる名前付き関数を渡してください".to_owned());
                    };
                    let helper = OptionMapHelper {
                        input: self.module.locals[*function.params.first().ok_or_else(|| "Option.map の引数定義が不正です".to_owned())?].ty,
                        output: function.ret,
                        callback: *callback,
                    };
                    if !self.map_helpers.contains(&helper) {
                        return Err("Option.map の C 変換用関数が見つかりません".to_owned());
                    }
                    let input = args.first().ok_or_else(|| "Option.map の入力引数がありません".to_owned())?;
                    return Ok((format!("{}({})", helper.name(), self.value(input)?), function.ret));
                }
                let mut call_args = Vec::new();
                if function.params.len() != args.len() {
                    return Err(format!(
                        "関数 `{}` の呼び出し引数個数が不正です",
                        function.name
                    ));
                }
                for (&param, arg) in function.params.iter().zip(args) {
                    let ty = self.module.locals[param].ty;
                    if matches!(self.module.ty(ty), Type::Evidence) {
                        if !matches!(arg, Expr::Var(_) | Expr::Lit(Lit::Unit)) {
                            return Err("handler提供を含む関数呼び出しは未対応です".to_owned());
                        }
                        continue;
                    }
                    if matches!(self.module.ty(ty), Type::Unit) {
                        // 表現を持たない値でも、呼び出し引数としては副作用の評価順を保つ。
                        self.value(arg)?;
                        continue;
                    }
                    call_args.push(self.value(arg)?);
                }
                Ok((
                    format!(
                        "{}({})",
                        function_name(*f, function),
                        call_args.join(", ")
                    ),
                    function.ret,
                ))
            }
            Callee::Indirect(_) => Err("組み込みCの初期実装では関数値の呼び出しを出力できません".to_owned()),
        }
    }

    fn value(&self, expr: &Expr) -> Result<String, String> {
        match expr {
            Expr::Lit(Lit::Int(n)) => Ok(if *n == i32::MIN {
                "(-2147483647 - 1)".to_owned()
            } else {
                n.to_string()
            }),
            Expr::Lit(Lit::Int64(n)) => Ok(if *n == i64::MIN {
                "(-INT64_C(9223372036854775807) - 1)".to_owned()
            } else if *n < 0 {
                format!("(-INT64_C({}))", n.unsigned_abs())
            } else {
                format!("INT64_C({n})")
            }),
            Expr::Lit(Lit::Bool(value)) => Ok(if *value { "true" } else { "false" }.to_owned()),
            Expr::Lit(Lit::Unit) => Ok("0".to_owned()),
            Expr::Lit(Lit::Float(_)) => Err("組み込みCの初期実装では Float を出力できません".to_owned()),
            Expr::Lit(Lit::String(_)) => Err("組み込みCの初期実装では String を出力できません".to_owned()),
            Expr::Var(local) => {
                if matches!(self.module.ty(self.module.locals[*local].ty), Type::Unit) {
                    Ok("0".to_owned())
                } else {
                    Ok(local_name(*local))
                }
            }
            Expr::Ctor { ctor, ty_args, args } => self.constructor(*ctor, ty_args, args),
            Expr::Field { base, ctor, index } => {
                let def = &self.module.enums[ctor.enum_id];
                let variant = &def.variants[ctor.variant];
                if def.name.ends_with(".GpioPin") && *index == 0 {
                    self.value(base)
                } else if let VariantFields::Named(fields) = &variant.fields {
                    let field = fields.get(*index).ok_or_else(|| "field index out of range".to_owned())?;
                    Ok(format!("({}).{}", self.value(base)?, c_field(field)))
                } else {
                    Err(format!("field access on `{}` is not supported", def.name))
                }
            }
            Expr::Binary { op, ty, lhs, rhs } => {
                let scalar_enum_equality = same_scalar_enum(self.module, lhs, rhs);
                let lhs = self.value(lhs)?;
                let rhs = self.value(rhs)?;
                let helper = match (op, ty) {
                    (emela_core::BinOp::Add, emela_core::OpTy::Int) => Some("emela_i32_add"),
                    (emela_core::BinOp::Sub, emela_core::OpTy::Int) => Some("emela_i32_sub"),
                    (emela_core::BinOp::Mul, emela_core::OpTy::Int) => Some("emela_i32_mul"),
                    (emela_core::BinOp::Add, emela_core::OpTy::Int64) => Some("emela_i64_add"),
                    (emela_core::BinOp::Sub, emela_core::OpTy::Int64) => Some("emela_i64_sub"),
                    (emela_core::BinOp::Mul, emela_core::OpTy::Int64) => Some("emela_i64_mul"),
                    _ => None,
                };
                if let Some(helper) = helper {
                    return Ok(format!("{helper}({lhs}, {rhs})"));
                }
                if matches!(op, emela_core::BinOp::Div | emela_core::BinOp::Rem) {
                    return Err("除算と剰余は初期実装では未対応です".to_owned());
                }
                if matches!(ty, emela_core::OpTy::String) {
                    return Err("文字列の比較は未対応です".to_owned());
                }
                if matches!(ty, emela_core::OpTy::Structural)
                    && !scalar_enum_equality
                {
                    return Err("構造体の比較は未対応です".to_owned());
                }
                let op = match op {
                    emela_core::BinOp::Eq => "==",
                    emela_core::BinOp::Ne => "!=",
                    emela_core::BinOp::Lt => "<",
                    emela_core::BinOp::Le => "<=",
                    emela_core::BinOp::Gt => ">",
                    emela_core::BinOp::Ge => ">=",
                    emela_core::BinOp::And => "&&",
                    emela_core::BinOp::Or => "||",
                    _ => return Err("この整数演算は未対応です".to_owned()),
                };
                Ok(format!("({lhs} {op} {rhs})"))
            }
            Expr::Unary { op, ty, operand } => {
                let operand = self.value(operand)?;
                match (op, ty) {
                    (emela_core::UnOp::Neg, emela_core::OpTy::Int) => {
                        Ok(format!("emela_i32_neg({operand})"))
                    }
                    (emela_core::UnOp::Neg, emela_core::OpTy::Int64) => {
                        Ok(format!("emela_i64_neg({operand})"))
                    }
                    (emela_core::UnOp::Not, _) => Ok(format!("(!{operand})")),
                    _ => Err("この単項演算は未対応です".to_owned()),
                }
            }
            Expr::If { cond, then, else_ } => Ok(format!(
                "({} ? {} : {})",
                self.value(cond)?,
                self.value(then)?,
                self.value(else_)?
            )),
            Expr::Call { .. } => self.call(expr).map(|(call, _)| call),
            _ => Err(format!("組み込みCの初期実装では `{}` を出力できません", expr_kind(expr))),
        }
    }

    fn constructor(
        &self,
        ctor: emela_core::CtorRef,
        ty_args: &[TypeId],
        args: &[Expr],
    ) -> Result<String, String> {
        let def = &self.module.enums[ctor.enum_id];
        let variant = def
            .variants
            .get(ctor.variant)
            .ok_or_else(|| "constructor index out of range".to_owned())?;
        let values = args
            .iter()
            .map(|arg| self.value(arg))
            .collect::<Result<Vec<_>, _>>()?;
        if is_named(&def.name, "Option") {
            let option_ty = self
                .module
                .types
                .get(&Type::Enum(ctor.enum_id, ty_args.to_vec()))
                .ok_or_else(|| "Option constructor type is missing".to_owned())?;
            let c_ty = self.c_type(option_ty)?;
            return match ctor.variant {
                0 if values.len() == 1 => Ok(format!(
                    "({c_ty}){{ .is_some = true, .value = {} }}",
                    values[0]
                )),
                1 if values.is_empty() => Ok(format!("({c_ty}){{ .is_some = false }}")),
                _ => Err("Option constructor is malformed".to_owned()),
            };
        }
        if def.name.ends_with(".GpioPin") && values.len() == 1 {
            return Ok(values[0].clone());
        }
        if def.name.ends_with(".GpioEvent") {
            if let VariantFields::Named(fields) = &variant.fields {
                if fields.len() == values.len() {
                    let fields = fields
                        .iter()
                        .zip(values)
                        .map(|(field, value)| format!(".{} = {value}", c_field(field)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Ok(format!("(EmelaGpioEvent){{ {fields} }}"));
                }
            }
            return Err("GpioEvent constructor is malformed".to_owned());
        }
        if !values.is_empty() {
            return Err(format!("payload constructor `{}` is not supported", variant.name));
        }
        Ok(format!("((uint8_t){})", ctor.variant))
    }

    fn c_type(&self, id: TypeId) -> Result<String, String> {
        match self.module.ty(id) {
            Type::Int => Ok("int32_t".to_owned()),
            Type::Int64 => Ok("int64_t".to_owned()),
            Type::Bool => Ok("bool".to_owned()),
            Type::Unit => Ok("void".to_owned()),
            Type::Enum(enum_id, _args) => {
                let def = &self.module.enums[*enum_id];
                if def.name.ends_with(".GpioPin") {
                    Ok("EmelaGpioPin".to_owned())
                } else if def.name.ends_with(".PinState") {
                    Ok("EmelaPinState".to_owned())
                } else if def.name.ends_with(".Pull") {
                    Ok("EmelaGpioPull".to_owned())
                } else if def.name.ends_with(".GpioMode") {
                    Ok("EmelaGpioMode".to_owned())
                } else if def.name.ends_with(".Edge") {
                    Ok("EmelaGpioEdge".to_owned())
                } else if def.name.ends_with(".GpioEvent") {
                    Ok("EmelaGpioEvent".to_owned())
                } else if option_payload(self.module, id).is_some() {
                    Ok(option_type_name(id))
                } else if def.variants.iter().all(|variant| variant.tys.is_empty()) {
                    Ok("uint8_t".to_owned())
                } else if def.variants.len() == 1 && def.variants[0].tys.len() == 1 {
                    self.c_type(def.variants[0].tys[0])
                } else {
                    Err(format!("型 `{}` のC表現は未対応です", def.name))
                }
            }
            Type::Float => Err("組み込みCの初期実装では Float を出力できません".to_owned()),
            Type::String => Err("組み込みCの初期実装では String を出力できません".to_owned()),
            Type::Never => Ok("void".to_owned()),
            Type::Evidence => Ok("void *".to_owned()),
            other => Err(format!("型 `{other:?}` のC表現は未対応です")),
        }
    }

    fn option_storage_type(&self, payload: TypeId) -> Result<String, String> {
        let ty = self.c_type(payload)?;
        Ok(if ty == "void" { "uint8_t".to_owned() } else { ty })
    }

    fn line(&mut self, indent: usize, line: &str) {
        for _ in 0..indent {
            self.out.push_str("  ");
        }
        self.out.push_str(line);
        self.out.push('\n');
    }
}

fn expr_kind(expr: &Expr) -> &'static str {
    match expr {
        Expr::Let { .. } => "let",
        Expr::Call { .. } => "call",
        Expr::Builtin { .. } => "builtin",
        Expr::Tuple(_) => "tuple",
        Expr::List { .. } => "list",
        Expr::Match { .. } => "match",
        Expr::Lambda { .. } => "lambda",
        Expr::Concat(_) => "string interpolation",
        Expr::Loop { .. } => "loop",
        Expr::Recur(_) => "recur",
        Expr::Assert(_) => "assert",
        Expr::Fail(_) => "fail",
        Expr::Escape { .. } => "escape",
        Expr::Use { .. } => "use",
        Expr::Provide { .. } => "provide",
        Expr::Handler { .. } => "handler",
        Expr::With { .. } => "with",
        Expr::Fiber { .. } => "fiber operation",
        Expr::Lit(_) => "literal",
        Expr::Var(_) => "variable",
        Expr::Fn { .. } => "function value",
        Expr::Ctor { .. } => "constructor",
        Expr::Field { .. } => "field access",
        Expr::If { .. } => "if",
        Expr::Binary { .. } => "binary operation",
        Expr::Unary { .. } => "unary operation",
    }
}

fn local_name(local: emela_core::Local) -> String {
    format!("v{}", u32::from(local.into_raw()))
}

fn function_name(id: emela_core::FnId, function: &Function) -> String {
    if function.exported && function.name != "main" {
        return format!("emela_export_{}", c_field(&function.name));
    }
    let mut out = format!("emela_f{}", u32::from(id.into_raw()));
    out.push('_');
    out.extend(function.name.chars().take(32).map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }));
    out
}

fn option_type_name(id: TypeId) -> String {
    format!("EmelaOption_{}", id.index())
}

fn c_field(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() || out.as_bytes()[0].is_ascii_digit() {
        out.insert(0, '_');
    }
    out
}

fn is_named(qualified: &str, name: &str) -> bool {
    qualified == name || qualified.ends_with(&format!(".{name}"))
}
fn gpio_header(module: &Module, options: &[TypeId]) -> Result<String, String> {
    let mut out = String::from(
        "#ifndef EMELA_GPIO_H\n#define EMELA_GPIO_H\n\n#include <stdbool.h>\n#include <stdint.h>\n\n/* 論理ピン番号。物理ピンとの対応はボード側で決めます。 */\ntypedef int32_t EmelaGpioPin;\ntypedef uint8_t EmelaPinState;\ntypedef uint8_t EmelaGpioPull;\ntypedef uint8_t EmelaGpioMode;\ntypedef uint8_t EmelaGpioEdge;\ntypedef struct { EmelaGpioPin pin; EmelaPinState level; } EmelaGpioEvent;\n\n",
    );
    let mut ordered = options.to_vec();
    ordered.sort_by_key(|&ty| option_depth(module, ty));
    for ty in ordered {
        let payload = option_payload(module, ty).ok_or_else(|| "Option型のペイロードがありません".to_owned())?;
        let payload_type = c_type(module, payload)?;
        let payload_type = if payload_type == "void" {
            "uint8_t".to_owned()
        } else {
            payload_type
        };
        writeln!(
            out,
            "typedef struct {{ bool is_some; {payload_type} value; }} {};",
            option_type_name(ty)
        )
        .unwrap();
    }
    out.push_str(
        "\nenum { EMELA_PIN_STATE_LOW = 0, EMELA_PIN_STATE_HIGH = 1 };\nenum { EMELA_GPIO_PULL_NO_PULL = 0, EMELA_GPIO_PULL_UP = 1, EMELA_GPIO_PULL_DOWN = 2 };\nenum { EMELA_GPIO_MODE_INPUT = 0, EMELA_GPIO_MODE_OUTPUT_PUSH_PULL = 1, EMELA_GPIO_MODE_OUTPUT_OPEN_DRAIN = 2, EMELA_GPIO_MODE_ANALOG = 3 };\nenum { EMELA_GPIO_EDGE_RISING = 0, EMELA_GPIO_EDGE_FALLING = 1, EMELA_GPIO_EDGE_BOTH = 2 };\n\n/* HALやレジスターアクセスはボード側が実装します。割り込み中に呼ぶ操作はISR安全に実装してください。 */\nvoid emela_gpio_configure(EmelaGpioPin pin, EmelaGpioMode mode, EmelaGpioPull pull);\nEmelaPinState emela_gpio_read(EmelaGpioPin pin);\nvoid emela_gpio_write(EmelaGpioPin pin, EmelaPinState state);\nvoid emela_gpio_toggle(EmelaGpioPin pin);\nvoid emela_gpio_enable_interrupt(EmelaGpioPin pin, EmelaGpioEdge edge);\nvoid emela_gpio_disable_interrupt(EmelaGpioPin pin);\n\n#endif\n",
    );
    Ok(out)
}

fn option_depth(module: &Module, ty: TypeId) -> usize {
    option_payload(module, ty).map_or(0, |payload| 1 + option_depth(module, payload))
}

fn c_type(module: &Module, id: TypeId) -> Result<String, String> {
    match module.ty(id) {
        Type::Int => Ok("int32_t".to_owned()),
        Type::Int64 => Ok("int64_t".to_owned()),
        Type::Bool => Ok("bool".to_owned()),
        Type::Unit => Ok("void".to_owned()),
        Type::Enum(enum_id, _) => {
            let def = &module.enums[*enum_id];
            if def.name.ends_with(".GpioPin") {
                Ok("EmelaGpioPin".to_owned())
            } else if def.name.ends_with(".PinState") {
                Ok("EmelaPinState".to_owned())
            } else if def.name.ends_with(".Pull") {
                Ok("EmelaGpioPull".to_owned())
            } else if def.name.ends_with(".GpioMode") {
                Ok("EmelaGpioMode".to_owned())
            } else if def.name.ends_with(".Edge") {
                Ok("EmelaGpioEdge".to_owned())
            } else if def.name.ends_with(".GpioEvent") {
                Ok("EmelaGpioEvent".to_owned())
            } else if option_payload(module, id).is_some() {
                Ok(option_type_name(id))
            } else if def.variants.iter().all(|variant| variant.tys.is_empty()) {
                Ok("uint8_t".to_owned())
            } else if def.variants.len() == 1 && def.variants[0].tys.len() == 1 {
                c_type(module, def.variants[0].tys[0])
            } else {
                Err(format!("型 `{}` のC表現は未対応です", def.name))
            }
        }
        Type::Float => Err("組み込みCの初期実装では Float を出力できません".to_owned()),
        Type::String => Err("組み込みCの初期実装では String を出力できません".to_owned()),
        Type::Never => Ok("void".to_owned()),
        Type::Evidence => Ok("void *".to_owned()),
        other => Err(format!("型 `{other:?}` のC表現は未対応です")),
    }
}

fn app_header(
    module: &Module,
    functions: &[emela_core::FnId],
    emitter: &Emitter<'_>,
) -> Result<String, Vec<Error>> {
    let mut out = String::from(
        "#ifndef EMELA_EMBEDDED_H\n#define EMELA_EMBEDDED_H\n\n#include \"emela_gpio.h\"\n\n",
    );
    for &id in functions {
        let function = &module.functions[id];
        if !function.exported || function.name == "main" {
            continue;
        }
        let signature = emitter
            .function_signature(id, function)
            .map_err(|message| vec![error(message)])?;
        writeln!(out, "{signature};").unwrap();
    }
    out.push_str("\n#endif\n");
    Ok(out)
}

fn error(message: impl Into<String>) -> Error {
    Error {
        message: message.into(),
    }
}
