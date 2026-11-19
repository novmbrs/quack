//! Components that belong to an optional module (§13.1.6): routed to the
//! registered [`crate::Module`] that owns them, after the ack, with the
//! clicker's principal built from the payload.

use quack_discord::custom_id::CustomId;
use quack_discord::interaction::{FieldValue, SelectChoice, Source};
use quack_text::actions::InputId;

use crate::ack::{Finish, Pending};
use crate::auth;
use crate::modules::ComponentCall;
use crate::router::expired;

/// Routes a module's component or modal submit.
pub(crate) async fn component(
    p: Pending<'_>,
    id: CustomId,
    values: &[SelectChoice],
    fields: &[(InputId, FieldValue)],
    source: Source,
) -> Finish {
    let module = p.shared().modules.iter().find(|m| m.owns(&id)).cloned();
    let (Some(module), Some(actor)) = (module, p.ix.invoker.as_ref().map(|i| i.account.id)) else {
        return p.fail(expired()).await;
    };
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let principal = match a.ix.guild_id {
        Some(_) => auth::principal(a.engine(), a.ix).await.ok(),
        None => None,
    };
    let call = ComponentCall {
        guild: a.ix.guild_id,
        channel: a.ix.channel_id,
        actor,
        principal,
        interaction_id: a.ix.id,
        id,
        values,
        fields,
        source,
    };
    let result = module.on_component(a.engine(), call).await;
    a.finish(result).await
}
