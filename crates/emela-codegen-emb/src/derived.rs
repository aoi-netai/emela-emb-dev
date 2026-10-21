//! Finite derivation requests for expanding generic enum families.
use std::collections::HashSet;

use emela_core::{EnumId, External, ExternalBinding, HostRef, Module, Type, TypeId};
use emela_lir::{DeriveKind, Deriver, FnBuilder, Function};

pub(crate) const TARGET: &str = "emela-emb-derived";

pub(crate) struct BackendDeriver {
    expanding: HashSet<EnumId>,
}

impl BackendDeriver {
    pub(crate) fn new(core: &Module) -> Self {
        Self {
            expanding: crate::type_expansion::expanding_enums(core),
        }
    }

    fn contains(&self, core: &Module, ty: TypeId, seen: &mut HashSet<EnumId>) -> bool {
        match core.ty(ty) {
            Type::List(t) | Type::Cell(t) => self.contains(core, *t, seen),
            Type::Tuple(fields) => fields.iter().any(|&t| self.contains(core, t, seen)),
            Type::Enum(id, args) => {
                self.expanding.contains(id)
                    || args.iter().any(|&t| self.contains(core, t, seen))
                    || (seen.insert(*id)
                        && core.enums[*id]
                            .variants
                            .iter()
                            .flat_map(|v| &v.tys)
                            .any(|&t| self.contains(core, t, seen)))
            }
            _ => false,
        }
    }
}

impl Deriver for BackendDeriver {
    fn derive(
        &mut self,
        core: &mut Module,
        kind: DeriveKind,
        ty: TypeId,
        name: &str,
    ) -> Option<Function> {
        if kind == DeriveKind::ErrorName {
            return None;
        }
        if !self.contains(core, ty, &mut HashSet::new()) {
            return None;
        }
        let mut b = FnBuilder::new(core, name);
        b.param("a", ty);
        let symbol = match kind {
            DeriveKind::Eq => {
                b.param("b", ty);
                b.set_ret(TypeId::BOOL);
                "eq"
            }
            DeriveKind::Show => {
                b.set_ret(TypeId::STRING);
                "show"
            }
            DeriveKind::ErrorName => unreachable!("error names use the ordinary Flow derivation"),
        };
        Some(b.finish_external(External {
            bindings: vec![ExternalBinding {
                target: TARGET.into(),
                host: HostRef::Global(symbol.into()),
            }],
        }))
    }
}

pub(crate) fn kind(function: &Function) -> Option<DeriveKind> {
    let binding = function.external.as_ref()?.binding(TARGET)?;
    match &binding.host {
        HostRef::Global(s) if s == "eq" => Some(DeriveKind::Eq),
        HostRef::Global(s) if s == "show" => Some(DeriveKind::Show),
        _ => None,
    }
}

pub(crate) fn supported(core: &Module, ty: TypeId, kind: DeriveKind) -> bool {
    if kind == DeriveKind::ErrorName {
        return false;
    }
    fn go(
        core: &Module,
        ty: TypeId,
        env: &[bool],
        seen: &mut HashSet<(EnumId, Vec<bool>)>,
        functions: bool,
    ) -> bool {
        match core.ty(ty) {
            Type::Int
            | Type::Int64
            | Type::Float
            | Type::Bool
            | Type::String
            | Type::Unit
            | Type::Never => true,
            // Debug display never inspects a closure or changes its ownership.
            Type::Fn(_) => functions,
            Type::Param(i) => env.get(*i).copied().unwrap_or(false),
            Type::List(t) => go(core, *t, env, seen, functions),
            Type::Tuple(ts) => ts.iter().all(|&t| go(core, t, env, seen, functions)),
            Type::Enum(id, args) => {
                let flags: Vec<_> = args
                    .iter()
                    .map(|&t| go(core, t, env, seen, functions))
                    .collect();
                let key = (*id, flags.clone());
                if !seen.insert(key.clone()) {
                    return true;
                }
                let result = core.enums[*id]
                    .variants
                    .iter()
                    .flat_map(|v| &v.tys)
                    .all(|&t| go(core, t, &flags, seen, functions));
                seen.remove(&key);
                result
            }
            _ => false,
        }
    }
    go(core, ty, &[], &mut HashSet::new(), kind == DeriveKind::Show)
}
