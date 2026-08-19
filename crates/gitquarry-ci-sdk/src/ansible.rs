use crate::job::JobBuilder;

/// The vault password's env key — injected via `.secret()`, never as a
/// plain env var value or CLI arg text (`ansible-vault` expects a file).
const VAULT_PASS_ENV: &str = "GITQUARRY_VAULT_PASS";
const VAULT_PASS_FIFO: &str = "$VAULT_PASS_FIFO";

pub trait AnsibleJobExt {
    fn ansible_playbook(self, playbook: impl Into<String>) -> AnsibleJobBuilder;
}

impl AnsibleJobExt for JobBuilder {
    fn ansible_playbook(self, playbook: impl Into<String>) -> AnsibleJobBuilder {
        AnsibleJobBuilder {
            job: self,
            playbook: playbook.into(),
            inventory: None,
            extra_vars: Vec::new(),
            vault_password_secret: None,
            tags: Vec::new(),
        }
    }
}

/// Ansible is not a third pipeline language or a new execution mode — this
/// builder just ends up producing a normal [`JobBuilder`] via [`finish`](Self::finish),
/// same contract, same executor.
#[must_use = "call .finish() to turn this back into a JobBuilder"]
pub struct AnsibleJobBuilder {
    job: JobBuilder,
    playbook: String,
    inventory: Option<String>,
    extra_vars: Vec<(String, String)>,
    vault_password_secret: Option<String>,
    tags: Vec<String>,
}

impl AnsibleJobBuilder {
    pub fn inventory(mut self, path: impl Into<String>) -> Self {
        self.inventory = Some(path.into());
        self
    }

    pub fn extra_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_vars.push((key.into(), value.into()));
        self
    }

    pub fn vault_password_secret(mut self, secret_name: impl Into<String>) -> Self {
        self.vault_password_secret = Some(secret_name.into());
        self
    }

    pub fn tags(mut self, tags: impl Into<String>) -> Self {
        self.tags.push(tags.into());
        self
    }

    pub fn finish(mut self) -> JobBuilder {
        let mut command = String::from("ansible-playbook");

        if let Some(inventory) = &self.inventory {
            command.push_str(" --inventory ");
            command.push_str(&shell_escape(inventory));
        }
        for (key, value) in &self.extra_vars {
            command.push_str(" --extra-vars ");
            command.push_str(&shell_escape(&format!("{key}={value}")));
        }
        if !self.tags.is_empty() {
            command.push_str(" --tags ");
            command.push_str(&shell_escape(&self.tags.join(",")));
        }
        if let Some(secret_name) = self.vault_password_secret.take() {
            self.job = self.job.secret(VAULT_PASS_ENV, secret_name);
            command.push_str(" --vault-password-file ");
            command.push_str(VAULT_PASS_FIFO);
        }
        command.push(' ');
        command.push_str(&shell_escape(&self.playbook));

        self.job.run(command)
    }
}

/// Minimal shlex-style escaping for values (e.g. [`PipelineContext::sha`])
/// interpolated into a shell command string. Not a full shlex
/// implementation — just enough to make every value round-trip through
/// `/bin/sh` as a single argument.
pub fn shell_escape(value: &str) -> String {
    let is_plain = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@%+=,".contains(c));

    if is_plain {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}
