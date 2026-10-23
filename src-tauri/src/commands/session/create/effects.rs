use crate::session::selection::LifecycleEffectGuard;
use crate::session::selection::LifecycleEffectSlot;
use crate::session::selection::OwnedReplacementContinuation;
use crate::session::selection::SelectionTransaction;

/// Outcome of starting one create effect.
pub(crate) enum CreateEffectStart {
    /// The effect may run; the guard is `None` when no lifecycle owner exists.
    Started(Option<LifecycleEffectGuard>),
    /// A lifecycle owner exists but did not grant the effect: the create
    /// retains instead of running it.
    Retain,
}

/// Starts the create effect `slot` under the inline lifecycle entry, or else
/// under the owned replacement continuation.
pub(crate) fn start_create_effect<R: tauri::Runtime>(
    inline_transaction: Option<&SelectionTransaction<R>>,
    owned_replacement: Option<&OwnedReplacementContinuation>,
    slot: LifecycleEffectSlot,
) -> CreateEffectStart {
    let effect = inline_transaction
        .and_then(|transaction| transaction.start_lifecycle_effect(slot))
        .or_else(|| owned_replacement.and_then(|continuation| continuation.start_effect(slot)));
    if (inline_transaction.is_some_and(|transaction| transaction.has_lifecycle_entry())
        || owned_replacement.is_some())
        && effect.is_none()
    {
        return CreateEffectStart::Retain;
    }
    CreateEffectStart::Started(effect)
}
