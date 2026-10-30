use crate::json::{json, Value};
use crate::model::Dock;
use crate::tabs::Tabs;

pub const MAC_APP: &str = "mac";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Where {
    Desk(String),
    Backlog,
    Planner(String),
}

impl Where {
    pub fn to_json(&self) -> Value {
        match self {
            Where::Desk(app) => json!({ "desk": app }),
            Where::Backlog => "shelf".into(),
            Where::Planner(app) => json!({ "planner": app }),
        }
    }

    pub fn read(value: Option<&Value>) -> Option<Where> {
        match value? {
            Value::String(place) if place == "shelf" => Some(Where::Backlog),
            Value::Object(place) => {
                let app = |key: &str| place.get(key).and_then(Value::as_str).map(String::from);
                app("desk").map(Where::Desk).or_else(|| app("planner").map(Where::Planner))
            }
            _ => None,
        }
    }

    pub fn upgrade(value: &Value) -> Value {
        match value {
            Value::String(place) if place == "shelf" => json!({ "desk": MAC_APP }),
            Value::String(place) if place == "backlog" => "shelf".into(),
            Value::Object(place) if place.contains_key("shelf") => json!({ "desk": place.get("shelf").cloned().unwrap_or(Value::Null) }),
            Value::Object(place) if place.contains_key("desk") => json!({ "desk": MAC_APP }),
            _ => value.clone(),
        }
    }

    pub fn app(&self) -> Option<&str> {
        match self {
            Where::Desk(app) | Where::Planner(app) => Some(app),
            Where::Backlog => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    pub id: String,
    pub dock: Dock,
    pub tabs: Tabs,
    pub place: Where,
    pub left_at: Option<f64>,
}

impl Workspace {
    pub(crate) fn new(id: String, dock: Dock, place: Where) -> Workspace {
        Workspace { id, dock, tabs: Tabs::default(), place, left_at: None }
    }

    pub fn is_task(&self) -> bool {
        !matches!(self.place, Where::Planner(_))
    }

    pub fn summary(&self) -> Value {
        json!({ "id": self.id, "where": self.place.to_json() })
    }
}
