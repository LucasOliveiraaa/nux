//! Variable scopes and the versions being declared.

use std::collections::HashMap;

use nux_lexer::Span;

use crate::{EvalError, Result, Value, VersionDef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ScopeId(u32);

#[derive(Debug, Clone)]
pub(crate) struct Scope {
    pub bindings: HashMap<String, Value>,
    pub parent: Option<ScopeId>,
}

pub(crate) struct EvalContext {
    pub next_scope_id: u32,
    pub scopes: HashMap<ScopeId, Scope>,
    pub current_scope: ScopeId,

    /// Versions closed by a later `version(...)` call.
    pub versions: Vec<VersionDef>,
    /// The version that declarations currently belong to.
    pub current: Option<VersionDef>,
}

impl EvalContext {
    pub(crate) fn new() -> Self {
        let mut scopes = HashMap::new();
        let root_scope_id = ScopeId(0);
        scopes.insert(
            root_scope_id,
            Scope {
                bindings: HashMap::new(),
                parent: None,
            },
        );

        let mut context = Self {
            next_scope_id: 1,
            scopes,
            current_scope: root_scope_id,

            versions: Vec::new(),
            current: None,
        };
        context.set_binding("nux".to_string(), host_object());
        context
    }

    pub(crate) fn enter_scope(&mut self) {
        let new_scope_id = ScopeId(self.next_scope_id);
        self.next_scope_id += 1;

        let new_scope = Scope {
            bindings: HashMap::new(),
            parent: Some(self.current_scope),
        };

        self.scopes.insert(new_scope_id, new_scope);
        self.current_scope = new_scope_id;
    }

    pub(crate) fn exit_scope(&mut self) {
        // Scopes are strictly nested, so the exited scope can never be
        // reached again; drop it instead of keeping it alive forever.
        if let Some(parent_scope_id) = self.scopes[&self.current_scope].parent {
            self.scopes.remove(&self.current_scope);
            self.current_scope = parent_scope_id;
        }
    }

    pub(crate) fn set_binding(&mut self, name: String, value: Value) {
        if let Some(current_scope) = self.scopes.get_mut(&self.current_scope) {
            current_scope.bindings.insert(name, value);
        }
    }

    pub(crate) fn find_scope(&self, name: &str) -> Option<ScopeId> {
        let mut scope_id = Some(self.current_scope);

        while let Some(id) = scope_id {
            let scope = self.scopes.get(&id)?;
            if scope.bindings.contains_key(name) {
                return Some(id);
            }
            scope_id = scope.parent;
        }

        None
    }

    pub(crate) fn get_binding(&self, name: &str) -> Option<&Value> {
        let id = self.find_scope(name)?;
        self.scopes[&id].bindings.get(name)
    }

    /// Updates the nearest existing binding of `name`. Returns `false` if
    /// there is none.
    pub(crate) fn assign(&mut self, name: &str, value: Value) -> bool {
        let Some(id) = self.find_scope(name) else {
            return false;
        };
        self.scopes
            .get_mut(&id)
            .unwrap()
            .bindings
            .insert(name.to_string(), value);
        true
    }

    /// Stores the current version, if any, so a new one can start.
    pub(crate) fn finish_version(&mut self) {
        if let Some(current) = self.current.take() {
            self.versions.push(current);
        }
    }

    pub(crate) fn has_version(&self, version: &str) -> bool {
        self.versions
            .iter()
            .chain(&self.current)
            .any(|v| v.version == version)
    }

    /// The version that `what` declares into; an error before any `version(...)`.
    pub(crate) fn current_version(&mut self, what: &str, span: Span) -> Result<&mut VersionDef> {
        self.current.as_mut().ok_or_else(|| {
            EvalError::new(
                format!("{what} must come after a `version(...)` call"),
                span,
            )
        })
    }
}

/// The `nux` object exposing facts about the host.
pub(crate) fn host_object() -> Value {
    Value::Object(HashMap::from([
        (
            "arch".to_string(),
            Value::String(std::env::consts::ARCH.to_string()),
        ),
        (
            "os".to_string(),
            Value::String(std::env::consts::OS.to_string()),
        ),
    ]))
}
