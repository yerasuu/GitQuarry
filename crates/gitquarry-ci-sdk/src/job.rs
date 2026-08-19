use std::collections::HashMap;

use crate::wire_generated::{JobKind, JobSpec, SecretRef};

/// Builds one [`JobSpec`]. Mutable, chained (`self` consumed and returned) —
/// the result lives seconds (a process that runs, dumps, and dies), so
/// immutability would add ceremony without real benefit here.
#[must_use = "a JobBuilder does nothing until passed to GraphBuilder::add_job"]
pub struct JobBuilder {
    spec: JobSpec,
}

pub fn job(id: impl Into<String>) -> JobBuilder {
    JobBuilder::new(id)
}

impl JobBuilder {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            spec: JobSpec {
                id: id.into(),
                image: String::new(),
                commands: Vec::new(),
                needs: Vec::new(),
                env: HashMap::new(),
                secrets: Vec::new(),
                artifacts: Vec::new(),
                kind: JobKind::default(),
            },
        }
    }

    pub fn image(mut self, image: impl Into<String>) -> Self {
        self.spec.image = image.into();
        self
    }

    pub fn run(mut self, command: impl Into<String>) -> Self {
        self.spec.commands.push(command.into());
        self
    }

    pub fn needs(mut self, id: impl Into<String>) -> Self {
        self.spec.needs.push(id.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.spec.env.insert(key.into(), value.into());
        self
    }

    /// The only entry point for secret data anywhere in this SDK — a
    /// reference by name, never a value. There is no method that accepts a
    /// secret's actual value; the value never passes through the pipeline
    /// resolution process.
    pub fn secret(mut self, env_key: impl Into<String>, secret_name: impl Into<String>) -> Self {
        self.spec.secrets.push(SecretRef {
            env_key: env_key.into(),
            secret_name: secret_name.into(),
        });
        self
    }

    pub fn artifact(mut self, path: impl Into<String>) -> Self {
        self.spec.artifacts.push(path.into());
        self
    }

    pub fn kind(mut self, kind: JobKind) -> Self {
        self.spec.kind = kind;
        self
    }

    pub(crate) fn into_spec(self) -> JobSpec {
        self.spec
    }
}
