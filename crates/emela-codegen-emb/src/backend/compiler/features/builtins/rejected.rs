use super::super::*;
pub(in crate::backend) fn show_nested(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(invalid_flow_ir(
        &context.function.name,
        "unlowered builtin ShowNested at stage Flow",
    ))
}

pub(in crate::backend) fn panic(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(invalid_flow_ir(
        &context.function.name,
        "unlowered builtin Panic at stage Flow",
    ))
}

pub(in crate::backend) fn todo(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(invalid_flow_ir(
        &context.function.name,
        "unlowered builtin Todo at stage Flow",
    ))
}

pub(in crate::backend) fn dbg(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(invalid_flow_ir(
        &context.function.name,
        "unlowered builtin Dbg at stage Flow",
    ))
}

pub(in crate::backend) fn inspect(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(invalid_flow_ir(
        &context.function.name,
        "unlowered builtin Inspect at stage Flow",
    ))
}

pub(in crate::backend) fn show_string(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(invalid_flow_ir(
        &context.function.name,
        "unlowered builtin ShowString at stage Flow",
    ))
}

pub(in crate::backend) fn debug_print(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(invalid_flow_ir(
        &context.function.name,
        "unlowered builtin DebugPrint at stage Flow",
    ))
}
