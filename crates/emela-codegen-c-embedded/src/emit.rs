//! GPIO を含む直線的な Core IR を C11 に変換する。

use std::collections::VecDeque;
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
    let mut emitter = Emitter {
        module,
        functions: &functions,
        out: String::new(),
        errors: Vec::new(),
        temporary: 0,
    };
    emitter.write_header();
    emitter.write_prototypes();
    if !emitter.errors.is_empty() {
        return Err(emitter.errors);
    }
    for &id in &functions {
        emitter.write_function(id);
    }
    emitter.out.push_str("\nint main(void) {\n  ");
    let c_main = function_name(main_id, &module.functions[main_id].name);
    writeln!(emitter.out, "{c_main}();\n  return 0;\n}}").unwrap();
    if !emitter.errors.is_empty() {
        return Err(emitter.errors);
    }

    Ok(Output {
        main: emitter.out,
        gpio_header: gpio_header(),
    })
}

fn reachable_functions(module: &Module, main: emela_core::FnId) -> Result<Vec<emela_core::FnId>, Vec<Error>> {
    let mut reached = Vec::new();
    let mut queued = VecDeque::from([main]);
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
        collect_direct_calls(&function.body, &mut direct);
        queued.extend(direct);
    }
    Ok(reached)
}

fn collect_direct_calls(expr: &Expr, found: &mut Vec<emela_core::FnId>) {
    if let Expr::Call {
        callee: Callee::Direct { f, .. },
        ..
    } = expr
    {
        found.push(*f);
    }
    expr.for_each_child(&mut |child| collect_direct_calls(child, found));
}

struct Emitter<'a> {
    module: &'a Module,
    functions: &'a [emela_core::FnId],
    out: String,
    errors: Vec<Error>,
    temporary: usize,
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
        Ok(format!(
            "static {ret} {}({params})",
            function_name(id, &function.name)
        ))
    }

    fn write_function(&mut self, id: emela_core::FnId) {
        let function = self.module.functions[id].clone();
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
            Expr::Match { scrutinee, arms } => self.write_gpio_event_match(scrutinee, arms, ret, indent),
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

    fn write_gpio_event_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[emela_core::Arm],
        ret: TypeId,
        indent: usize,
    ) {
        let temp = format!("emela_match_{}", self.temporary);
        self.temporary += 1;
        let value = match scrutinee {
            Expr::Call { .. } => match self.call(scrutinee) {
                Ok((call, ty)) => match self.c_type(ty) {
                    Ok(ty) if ty == "EmelaGpioEventOption" => {
                        self.line(indent, &format!("{ty} {temp} = {call};"));
                        temp.clone()
                    }
                    Ok(_) => {
                        self.errors.push(error("GPIOイベントの照合対象は `Option[GpioEvent]` である必要があります"));
                        return;
                    }
                    Err(message) => {
                        self.errors.push(error(message));
                        return;
                    }
                },
                Err(message) => {
                    self.errors.push(error(message));
                    return;
                }
            },
            _ => {
                let value = match self.value(scrutinee) {
                    Ok(value) => value,
                    Err(message) => {
                        self.errors.push(error(message));
                        return;
                    }
                };
                self.line(
                    indent,
                    &format!("EmelaGpioEventOption {temp} = {value};"),
                );
                temp.clone()
            }
        };

        let mut some = None;
        let mut none = None;
        for arm in arms {
            if arm.guard.is_some() {
                self.errors.push(error("GPIOイベントの照合ではガード節を使えません"));
                return;
            }
            let Pat::Ctor { ctor, fields } = &arm.pat else {
                self.errors.push(error("GPIOイベントの照合は Some と None のパターンが必要です"));
                return;
            };
            let def = &self.module.enums[ctor.enum_id];
            if !is_named(&def.name, "Option") {
                self.errors.push(error("GPIOイベントの照合対象は `Option[GpioEvent]` である必要があります"));
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
            self.errors.push(error("GPIOイベントの照合には Some と None の両方が必要です"));
            return;
        };

        self.line(indent, &format!("if ({value}.is_some) {{"));
        match some_field {
            Pat::Bind(local) => {
                self.line(
                    indent + 1,
                    &format!("EmelaGpioEvent {} = {value}.value;", local_name(*local)),
                );
            }
            Pat::Wild => {}
            _ => {
                self.errors.push(error("Some の中では GpioEvent を変数へ束縛してください"));
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
                if !matches!(handler.as_ref(), Expr::Use { .. }) {
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
                    | "disable_interrupt" | "poll_interrupt" => {
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
                        function_name(*f, &function.name),
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
            Expr::Ctor { ctor, args, .. } => self.constructor(*ctor, args),
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
                if matches!(ty, emela_core::OpTy::Structural | emela_core::OpTy::String) {
                    return Err("構造体と文字列の比較は未対応です".to_owned());
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
            Expr::Call { .. } => Err("GPIO 呼び出しは let の値として使ってください".to_owned()),
            _ => Err(format!("組み込みCの初期実装では `{}` を出力できません", expr_kind(expr))),
        }
    }

    fn constructor(&self, ctor: emela_core::CtorRef, args: &[Expr]) -> Result<String, String> {
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
            return match ctor.variant {
                0 if values.len() == 1 => Ok(format!(
                    "(EmelaGpioEventOption){{ .is_some = true, .value = {} }}",
                    values[0]
                )),
                1 if values.is_empty() => Ok("(EmelaGpioEventOption){ .is_some = false }".to_owned()),
                _ => Err("GPIO event Option constructor is malformed".to_owned()),
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
            Type::Enum(enum_id, args) => {
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
                } else if is_named(&def.name, "Option")
                    && args.first().is_some_and(|arg| {
                        matches!(self.module.ty(*arg), Type::Enum(event, _) if self.module.enums[*event].name.ends_with(".GpioEvent"))
                    })
                {
                    Ok("EmelaGpioEventOption".to_owned())
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

fn function_name(id: emela_core::FnId, name: &str) -> String {
    let mut out = format!("emela_f{}", u32::from(id.into_raw()));
    out.push('_');
    out.extend(name.chars().take(32).map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }));
    out
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
fn gpio_header() -> String {
    "#ifndef EMELA_GPIO_H\n#define EMELA_GPIO_H\n\n#include <stdbool.h>\n#include <stdint.h>\n\n/* 論理ピン番号。物理ピンとの対応はボード側で決めます。 */\ntypedef int32_t EmelaGpioPin;\ntypedef uint8_t EmelaPinState;\ntypedef uint8_t EmelaGpioPull;\ntypedef uint8_t EmelaGpioMode;\ntypedef uint8_t EmelaGpioEdge;\ntypedef struct { EmelaGpioPin pin; EmelaPinState level; } EmelaGpioEvent;\ntypedef struct { bool is_some; EmelaGpioEvent value; } EmelaGpioEventOption;\n\nenum { EMELA_PIN_STATE_LOW = 0, EMELA_PIN_STATE_HIGH = 1 };\nenum { EMELA_GPIO_PULL_NO_PULL = 0, EMELA_GPIO_PULL_UP = 1, EMELA_GPIO_PULL_DOWN = 2 };\nenum { EMELA_GPIO_MODE_INPUT = 0, EMELA_GPIO_MODE_OUTPUT_PUSH_PULL = 1, EMELA_GPIO_MODE_OUTPUT_OPEN_DRAIN = 2, EMELA_GPIO_MODE_ANALOG = 3 };\nenum { EMELA_GPIO_EDGE_RISING = 0, EMELA_GPIO_EDGE_FALLING = 1, EMELA_GPIO_EDGE_BOTH = 2 };\n\n/* HALやレジスターアクセスはボード側が実装します。 */\nvoid emela_gpio_configure(EmelaGpioPin pin, EmelaGpioMode mode, EmelaGpioPull pull);\nEmelaPinState emela_gpio_read(EmelaGpioPin pin);\nvoid emela_gpio_write(EmelaGpioPin pin, EmelaPinState state);\nvoid emela_gpio_toggle(EmelaGpioPin pin);\nvoid emela_gpio_enable_interrupt(EmelaGpioPin pin, EmelaGpioEdge edge);\nvoid emela_gpio_disable_interrupt(EmelaGpioPin pin);\n/* 旧プロトタイプ用。最終設計の割り込み経路では使いません。 */\nEmelaGpioEventOption emela_gpio_poll_interrupt(void);\n\n#endif\n".to_owned()
}

fn error(message: impl Into<String>) -> Error {
    Error {
        message: message.into(),
    }
}
