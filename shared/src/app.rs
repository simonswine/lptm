use crux_core::{
    macros::effect,
    render::{render, RenderOperation},
    App, Command,
};
use crux_http::{command::Http, protocol::HttpRequest};
use serde::{Deserialize, Serialize};

#[effect]
pub enum Effect {
    Render(RenderOperation),
    Http(HttpRequest),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Datasource {
    pub id: u64,
    pub uid: String,
    pub name: String,
    #[serde(rename = "type")]
    pub ds_type: String,
    pub url: String,
    #[serde(rename = "isDefault")]
    pub is_default: bool,
    pub access: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum Event {
    Configure { url: String, token: String },
    FetchDatasources,
    SelectNext,
    SelectPrevious,
    DatasourcesLoaded(crux_http::Result<crux_http::Response<Vec<Datasource>>>),
    Quit,
}

#[derive(Default, Debug)]
pub struct Model {
    pub grafana_url: String,
    pub grafana_token: String,
    pub datasources: Vec<Datasource>,
    pub loading: bool,
    pub error: Option<String>,
    pub selected_index: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DatasourceView {
    pub name: String,
    pub ds_type: String,
    pub url: String,
    pub is_default: bool,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ViewModel {
    pub datasources: Vec<DatasourceView>,
    pub loading: bool,
    pub error: Option<String>,
    pub selected_index: usize,
}

#[derive(Default)]
pub struct ExploreTui;

impl App for ExploreTui {
    type Event = Event;
    type Model = Model;
    type ViewModel = ViewModel;
    type Capabilities = ();
    type Effect = Effect;

    fn update(
        &self,
        event: Self::Event,
        model: &mut Self::Model,
        _caps: &(),
    ) -> Command<Self::Effect, Self::Event> {
        match event {
            Event::Configure { url, token } => {
                model.grafana_url = url;
                model.grafana_token = token;
                Command::event(Event::FetchDatasources)
            }
            Event::FetchDatasources => {
                model.loading = true;
                model.error = None;
                let url = format!("{}/api/datasources", model.grafana_url);
                let token = model.grafana_token.clone();
                Command::all([
                    render(),
                    Http::<Effect, Event>::get(url)
                        .header("Authorization", format!("Bearer {token}"))
                        .expect_json::<Vec<Datasource>>()
                        .build()
                        .then_send(Event::DatasourcesLoaded),
                ])
            }
            Event::DatasourcesLoaded(Ok(mut response)) => {
                model.loading = false;
                model.datasources = response.take_body().unwrap_or_default();
                if !model.datasources.is_empty()
                    && model.selected_index >= model.datasources.len()
                {
                    model.selected_index = 0;
                }
                render()
            }
            Event::DatasourcesLoaded(Err(err)) => {
                model.loading = false;
                model.error = Some(err.to_string());
                render()
            }
            Event::SelectNext => {
                if !model.datasources.is_empty() {
                    model.selected_index =
                        (model.selected_index + 1) % model.datasources.len();
                }
                render()
            }
            Event::SelectPrevious => {
                if !model.datasources.is_empty() {
                    let len = model.datasources.len();
                    model.selected_index = (model.selected_index + len - 1) % len;
                }
                render()
            }
            Event::Quit => Command::done(),
        }
    }

    fn view(&self, model: &Self::Model) -> Self::ViewModel {
        ViewModel {
            datasources: model
                .datasources
                .iter()
                .map(|ds| DatasourceView {
                    name: ds.name.clone(),
                    ds_type: ds.ds_type.clone(),
                    url: ds.url.clone(),
                    is_default: ds.is_default,
                })
                .collect(),
            loading: model.loading,
            error: model.error.clone(),
            selected_index: model.selected_index,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crux_core::Core;
    use crux_http::testing::ResponseBuilder;

    fn make_core() -> Core<ExploreTui> {
        Core::new()
    }

    fn make_datasources() -> Vec<Datasource> {
        vec![
            Datasource {
                id: 1,
                uid: "uid1".into(),
                name: "Prometheus".into(),
                ds_type: "prometheus".into(),
                url: "http://prom:9090".into(),
                is_default: true,
                access: "proxy".into(),
            },
            Datasource {
                id: 2,
                uid: "uid2".into(),
                name: "Loki".into(),
                ds_type: "loki".into(),
                url: "http://loki:3100".into(),
                is_default: false,
                access: "proxy".into(),
            },
        ]
    }

    #[test]
    fn configure_triggers_fetch() {
        let core = make_core();
        let effects = core.process_event(Event::Configure {
            url: "http://localhost:3000".into(),
            token: "test-token".into(),
        });
        assert!(
            effects.iter().any(|e| matches!(e, Effect::Http(_))),
            "expected an Http effect after Configure"
        );
    }

    #[test]
    fn select_next_wraps_around() {
        let core = make_core();
        let ds = make_datasources();
        let response = ResponseBuilder::ok().body(ds).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));

        assert_eq!(core.view().selected_index, 0);

        core.process_event(Event::SelectNext);
        assert_eq!(core.view().selected_index, 1);

        // Wrap around
        core.process_event(Event::SelectNext);
        assert_eq!(core.view().selected_index, 0);
    }

    #[test]
    fn select_previous_wraps_around() {
        let core = make_core();
        let ds = make_datasources();
        let response = ResponseBuilder::ok().body(ds).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));

        // index is 0, SelectPrevious should wrap to 1
        core.process_event(Event::SelectPrevious);
        assert_eq!(core.view().selected_index, 1);
    }

    #[test]
    fn error_state_on_failed_load() {
        let core = make_core();
        core.process_event(Event::DatasourcesLoaded(Err(crux_http::HttpError::Url(
            "connection refused".into(),
        ))));
        let vm = core.view();
        assert!(vm.error.is_some(), "expected error to be set");
        assert!(!vm.loading, "expected loading to be false");
    }

    #[test]
    fn view_reflects_loaded_datasources() {
        let core = make_core();
        let ds = make_datasources();
        let response = ResponseBuilder::ok().body(ds).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));
        let vm = core.view();
        assert_eq!(vm.datasources.len(), 2);
        assert_eq!(vm.datasources[0].name, "Prometheus");
        assert!(vm.datasources[0].is_default);
    }
}
