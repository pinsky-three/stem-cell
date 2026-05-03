use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ProcessRunSpec {
    pub cwd: PathBuf,
    pub port: u16,
    pub command_chain: String,
}

impl ProcessRunSpec {
    pub fn new(cwd: impl Into<PathBuf>, port: u16, command_chain: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            port,
            command_chain: command_chain.into(),
        }
    }

    pub fn bash_script(&self) -> String {
        format!(
            "set -e && cd \"{dir}\" && export PORT={port} && \
             MISE=$( command -v mise || echo ~/.local/bin/mise ) && \
             {chain}",
            dir = self.cwd.display(),
            port = self.port,
            chain = self.command_chain,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerNetwork {
    Host,
    Bridge,
}

#[derive(Debug, Clone)]
pub struct ContainerRunSpec {
    pub runtime: String,
    pub image: String,
    pub memory_limit: String,
    pub network: ContainerNetwork,
    pub port: u16,
    pub script: String,
}

impl ContainerRunSpec {
    pub fn docker_args(&self) -> Vec<String> {
        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "-t".to_string(),
            format!("--memory={}", self.memory_limit),
            "--cap-drop=ALL".to_string(),
            "--security-opt=no-new-privileges".to_string(),
        ];
        match self.network {
            ContainerNetwork::Host => args.push("--network=host".to_string()),
            ContainerNetwork::Bridge => {
                args.push("-p".to_string());
                args.push(format!("127.0.0.1:{0}:{0}", self.port));
            }
        }
        args.push(self.image.clone());
        args.push("bash".to_string());
        args.push("-c".to_string());
        args.push(self.script.clone());
        args
    }
}

pub fn quote_path(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_script_sets_cwd_and_port() {
        let spec = ProcessRunSpec::new("/tmp/demo", 4242, "\"$MISE\" run dev");
        let script = spec.bash_script();
        assert!(script.contains("cd \"/tmp/demo\""));
        assert!(script.contains("export PORT=4242"));
    }

    #[test]
    fn bridge_container_maps_only_local_port() {
        let spec = ContainerRunSpec {
            runtime: "docker".into(),
            image: "debian".into(),
            memory_limit: "2g".into(),
            network: ContainerNetwork::Bridge,
            port: 4321,
            script: "echo ok".into(),
        };
        let args = spec.docker_args();
        assert!(args.contains(&"--cap-drop=ALL".to_string()));
        assert!(args.contains(&"127.0.0.1:4321:4321".to_string()));
    }
}
