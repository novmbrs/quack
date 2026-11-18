//! Components that belong to an optional module (§13.1.6): routed to the
//! registered [`crate::Module`] that owns them, after the ack. A modal the
//! module prefills goes through the same 300 ms exception as the core's.

use quack_discord::custom_id::CustomId;
use quack_discord::interaction::{FieldValue, SelectChoice, Source};
use quack_telemetry::labels::Reason;
use quack_text::actions::InputId;

use crate::ack::{Failure, Finish, Pending};
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
    let module = p
        .shared()
        .modules
        .iter()
        .find(|m| match id {
            CustomId::Setup(area, _) => area == m.setup_area(),
            _ => m.owns(&id),
        })
        .cloned();
    let (Some(module), Some(actor)) = (module, p.ix.invoker.as_ref().map(|i| i.account.id)) else {
        return p.fail(expired()).await;
    };
    if let Some(guild) = p.guild() {
        let shared = p.shared();
        if let Some(read) = module.prefilled_modal(&shared.engine, guild, actor, &id) {
            return p
                .prefilled_modal(|_| async move {
                    read.await
                        .map_err(|text| Failure::refused(text, Reason::NotAllowed))
                })
                .await;
        }
    }
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let call = ComponentCall {
        guild: a.ix.guild_id,
        channel: a.ix.channel_id,
        actor,
        interaction_id: a.ix.id,
        id,
        values,
        fields,
        source,
    };
    let msg = module.on_component(a.engine(), call).await;
    a.answer(&msg).await
}
