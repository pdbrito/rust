use rustc::hir::map::blocks::FnLikeNode;
use rustc::ty::query::Providers;
use rustc::ty::TyCtxt;
use rustc_hir as hir;
use rustc_hir::def_id::DefId;
use rustc_span::symbol::Symbol;
use rustc_target::spec::abi::Abi;
use syntax::attr;

/// Whether the `def_id` counts as const fn in your current crate, considering all active
/// feature gates
pub fn is_const_fn(tcx: TyCtxt<'_>, def_id: DefId) -> bool {
    tcx.is_const_fn_raw(def_id)
        && match is_unstable_const_fn(tcx, def_id) {
            Some(feature_name) => {
                // has a `rustc_const_unstable` attribute, check whether the user enabled the
                // corresponding feature gate.
                tcx.features().declared_lib_features.iter().any(|&(sym, _)| sym == feature_name)
            }
            // functions without const stability are either stable user written
            // const fn or the user is using feature gates and we thus don't
            // care what they do
            None => true,
        }
}

/// Whether the `def_id` is an unstable const fn and what feature gate is necessary to enable it
pub fn is_unstable_const_fn(tcx: TyCtxt<'_>, def_id: DefId) -> Option<Symbol> {
    if tcx.is_const_fn_raw(def_id) {
        let const_stab = tcx.lookup_const_stability(def_id)?;
        if const_stab.level.is_unstable() { Some(const_stab.feature) } else { None }
    } else {
        None
    }
}

/// Returns `true` if this function must conform to `min_const_fn`
pub fn is_min_const_fn(tcx: TyCtxt<'_>, def_id: DefId) -> bool {
    // Bail out if the signature doesn't contain `const`
    if !tcx.is_const_fn_raw(def_id) {
        return false;
    }

    if tcx.features().staged_api {
        // In order for a libstd function to be considered min_const_fn
        // it needs to be stable and have no `rustc_const_unstable` attribute.
        match tcx.lookup_const_stability(def_id) {
            // `rustc_const_unstable` functions don't need to conform.
            Some(&attr::ConstStability { ref level, .. }) if level.is_unstable() => false,
            None => {
                if let Some(stab) = tcx.lookup_stability(def_id) {
                    if stab.level.is_stable() {
                        tcx.sess.span_err(
                            tcx.def_span(def_id),
                            "stable const functions must have either `rustc_const_stable` or \
                             `rustc_const_unstable` attribute",
                        );
                        // While we errored above, because we don't know if we need to conform, we
                        // err on the "safe" side and require min_const_fn.
                        true
                    } else {
                        // Unstable functions need not conform to min_const_fn.
                        false
                    }
                } else {
                    // Internal functions are forced to conform to min_const_fn.
                    // Annotate the internal function with a const stability attribute if
                    // you need to use unstable features.
                    // Note: this is an arbitrary choice that does not affect stability or const
                    // safety or anything, it just changes whether we need to annotate some
                    // internal functions with `rustc_const_stable` or with `rustc_const_unstable`
                    true
                }
            }
            // Everything else needs to conform, because it would be callable from
            // other `min_const_fn` functions.
            _ => true,
        }
    } else {
        // users enabling the `const_fn` feature gate can do what they want
        !tcx.features().const_fn
    }
}

pub fn provide(providers: &mut Providers<'_>) {
    /// Const evaluability whitelist is here to check evaluability at the
    /// top level beforehand.
    fn is_const_intrinsic(tcx: TyCtxt<'_>, def_id: DefId) -> Option<bool> {
        if tcx.is_closure(def_id) {
            return None;
        }

        match tcx.fn_sig(def_id).abi() {
            Abi::RustIntrinsic | Abi::PlatformIntrinsic => {
                Some(tcx.lookup_const_stability(def_id).is_some())
            }
            _ => None,
        }
    }

    /// Checks whether the function has a `const` modifier or, in case it is an intrinsic, whether
    /// said intrinsic is on the whitelist for being const callable.
    fn is_const_fn_raw(tcx: TyCtxt<'_>, def_id: DefId) -> bool {
        let hir_id = tcx
            .hir()
            .as_local_hir_id(def_id)
            .expect("Non-local call to local provider is_const_fn");

        let node = tcx.hir().get(hir_id);

        if let Some(whitelisted) = is_const_intrinsic(tcx, def_id) {
            whitelisted
        } else if let Some(fn_like) = FnLikeNode::from_node(node) {
            fn_like.constness() == hir::Constness::Const
        } else if let hir::Node::Ctor(_) = node {
            true
        } else {
            false
        }
    }

    fn is_promotable_const_fn(tcx: TyCtxt<'_>, def_id: DefId) -> bool {
        is_const_fn(tcx, def_id)
            && match tcx.lookup_const_stability(def_id) {
                Some(stab) => {
                    if cfg!(debug_assertions) && stab.promotable {
                        let sig = tcx.fn_sig(def_id);
                        assert_eq!(
                            sig.unsafety(),
                            hir::Unsafety::Normal,
                            "don't mark const unsafe fns as promotable",
                            // https://github.com/rust-lang/rust/pull/53851#issuecomment-418760682
                        );
                    }
                    stab.promotable
                }
                None => false,
            }
    }

    fn const_fn_is_allowed_fn_ptr(tcx: TyCtxt<'_>, def_id: DefId) -> bool {
        is_const_fn(tcx, def_id)
            && tcx
                .lookup_const_stability(def_id)
                .map(|stab| stab.allow_const_fn_ptr)
                .unwrap_or(false)
    }

    *providers = Providers {
        is_const_fn_raw,
        is_promotable_const_fn,
        const_fn_is_allowed_fn_ptr,
        ..*providers
    };
}
