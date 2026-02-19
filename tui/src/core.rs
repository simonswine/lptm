use std::sync::Arc;

use shared::{Core, Effect, Event, ExploreTui};
use tokio::sync::mpsc;

pub struct AppCore {
    pub core: Arc<Core<ExploreTui>>,
    render_tx: mpsc::UnboundedSender<()>,
}

impl AppCore {
    pub fn new(render_tx: mpsc::UnboundedSender<()>) -> Self {
        Self {
            core: Arc::new(Core::new()),
            render_tx,
        }
    }

    pub fn update(&self, event: Event) {
        let effects = self.core.process_event(event);
        process_effects(effects, self.core.clone(), self.render_tx.clone());
    }
}

fn process_effects(
    effects: Vec<Effect>,
    core: Arc<Core<ExploreTui>>,
    render_tx: mpsc::UnboundedSender<()>,
) {
    for effect in effects {
        match effect {
            Effect::Render(_) => {
                let _ = render_tx.send(());
            }
            Effect::Http(mut request) => {
                let core = core.clone();
                let render_tx = render_tx.clone();
                tokio::spawn(async move {
                    let result = crate::http::execute(&request.operation).await;
                    match core.resolve(&mut request, result) {
                        Ok(new_effects) => {
                            process_effects(new_effects, core, render_tx);
                        }
                        Err(e) => eprintln!("resolve error: {e}"),
                    }
                });
            }
        }
    }
}
