resource_model_macro::resource_model_file!("../../specs/self.yaml");
system_model_macro::system_model_file!("../../specs/systems.yaml");

pub mod github_app;
pub mod github_webhook;
pub mod integrations;
pub mod proxy;
pub mod systems;
