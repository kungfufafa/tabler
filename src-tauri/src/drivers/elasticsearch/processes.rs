//! Running Queries over the task management API: `GET _tasks?detailed` as process rows, a task's
//! cancel reply as a kill outcome. Every request Tabler sends carries `x-opaque-id`
//! (`tabler-<session>`, a console run `tabler-<session>/<n>`), which the tasks it started repeat
//! in their `headers`: that is how Tabler's own tasks are told apart and a run's tasks found to
//! cancel them.

use chrono::{TimeZone, Utc};

use crate::types::admin::{KillOutcome, ProcessRow};

use super::errors;
use super::json::Json;

/// What every opaque id of Tabler starts with.
pub const OPAQUE_PREFIX: &str = "tabler-";

/// One task of a `_tasks` reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    /// `<node>:<number>`.
    pub id: String,
    pub node: String,
    pub action: String,
    pub description: Option<String>,
    pub start_millis: Option<i64>,
    pub running_nanos: Option<u64>,
    pub cancellable: bool,
    pub cancelled: bool,
    /// The `X-Opaque-Id` of the request that started it.
    pub opaque_id: Option<String>,
}

/// The tasks of `GET _tasks` (`group_by=none`: a `tasks` array; the default groups them by node).
pub fn parse_tasks(reply: &Json) -> Vec<Task> {
    let mut out = Vec::new();
    match reply.get("tasks") {
        Some(Json::Arr(tasks)) => out.extend(tasks.iter().filter_map(task)),
        Some(Json::Obj(tasks)) => out.extend(tasks.iter().filter_map(|(_, t)| task(t))),
        _ => {}
    }
    for (_, node) in reply
        .get("nodes")
        .and_then(Json::as_object)
        .unwrap_or_default()
    {
        for (_, t) in node
            .get("tasks")
            .and_then(Json::as_object)
            .unwrap_or_default()
        {
            out.extend(task(t));
        }
    }
    out.sort_by_key(|t| t.start_millis.unwrap_or(i64::MAX));
    out
}

fn task(t: &Json) -> Option<Task> {
    let node = t.get("node")?.as_str()?.to_string();
    let number = t.get("id")?.scalar_text()?;
    let opaque_id = t
        .get("headers")
        .and_then(Json::as_object)
        .unwrap_or_default()
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-opaque-id"))
        .and_then(|(_, v)| v.as_str())
        .map(str::to_string);
    Some(Task {
        id: format!("{node}:{number}"),
        node,
        action: t
            .get("action")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string(),
        description: t
            .get("description")
            .and_then(Json::as_str)
            .filter(|d| !d.is_empty())
            .map(str::to_string),
        start_millis: t.get("start_time_in_millis").and_then(Json::as_i64),
        running_nanos: t.get("running_time_in_nanos").and_then(Json::as_u64),
        cancellable: t.get("cancellable").and_then(Json::as_bool) == Some(true),
        cancelled: t.get("cancelled").and_then(Json::as_bool) == Some(true),
        opaque_id,
    })
}

/// The process rows of `tasks`, as seen by the session whose opaque id is `session`.
pub fn rows(tasks: &[Task], session: &str) -> Vec<ProcessRow> {
    tasks
        .iter()
        .map(|t| {
            let opaque = t.opaque_id.as_deref();
            let of_session =
                opaque.is_some_and(|o| o == session || o.starts_with(&format!("{session}/")));
            ProcessRow {
                id: t.id.clone(),
                user: None,
                database: None,
                application: t.opaque_id.clone(),
                client: Some(t.node.clone()),
                state: Some(
                    if t.cancelled {
                        "cancelled"
                    } else if t.cancellable {
                        "running"
                    } else {
                        "running (not cancellable)"
                    }
                    .to_string(),
                ),
                command: Some(t.action.clone()),
                started_at: t
                    .start_millis
                    .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
                    .map(|at| at.to_rfc3339()),
                duration_ms: t
                    .running_nanos
                    .map(|ns| i64::try_from(ns / 1_000_000).unwrap_or(i64::MAX)),
                wait_event: None,
                query: t.description.clone(),
                current: of_session && t.action.starts_with("cluster:monitor/tasks/lists"),
                own: opaque.is_some_and(|o| o.starts_with(OPAQUE_PREFIX)),
            }
        })
        .collect()
}

/// The tasks a run started (its opaque id), that can still be cancelled.
pub fn of_run<'a>(tasks: &'a [Task], opaque: &str) -> Vec<&'a Task> {
    tasks
        .iter()
        .filter(|t| t.opaque_id.as_deref() == Some(opaque) && t.cancellable && !t.cancelled)
        .collect()
}

/// The outcome of `POST _tasks/<id>/_cancel` (status and body).
pub fn kill_outcome(id: &str, status: u16, body: &[u8]) -> KillOutcome {
    let failed = |message: String| KillOutcome {
        id: id.to_string(),
        ok: false,
        message: Some(message),
    };
    if let Some(e) = errors::reply_error(crate::sqlgen::native::elastic::ENGINE, status, body) {
        return failed(e.to_string());
    }
    let reply = super::json::parse_bytes(body).unwrap_or(Json::Null);
    let failure = ["task_failures", "node_failures"].iter().find_map(|k| {
        reply
            .get(k)
            .and_then(Json::as_array)
            .and_then(|f| f.first())
            .map(|f| errors::reason_text(f.get("reason").unwrap_or(f)))
    });
    match failure {
        Some(why) => failed(why),
        None if !(200..300).contains(&status) => failed(format!("HTTP {status}")),
        None => KillOutcome {
            id: id.to_string(),
            ok: true,
            message: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::elasticsearch::json::parse;

    const TASKS: &str = r#"{"tasks":[
        {"node":"n1","id":464,"type":"transport","action":"indices:data/read/search","description":"indices[logs], source[{}]","start_time_in_millis":1700000000000,"running_time_in_nanos":13991383,"cancellable":true,"cancelled":false,"headers":{"X-Opaque-Id":"tabler-s1/3"}},
        {"node":"n1","id":465,"type":"transport","action":"cluster:monitor/tasks/lists","start_time_in_millis":1700000000100,"running_time_in_nanos":10,"cancellable":false,"headers":{"X-Opaque-Id":"tabler-s1"}},
        {"node":"n2","id":"9","action":"indices:data/write/update/byquery","cancellable":true,"headers":{}},
        {"node":"n2","action":"no id"}
    ]}"#;

    #[test]
    fn tasks_become_process_rows() {
        let tasks = parse_tasks(&parse(TASKS).unwrap());
        assert_eq!(tasks.len(), 3);
        let rows = rows(&tasks, "tabler-s1");
        assert_eq!(rows[0].id, "n1:464");
        assert_eq!(rows[0].application.as_deref(), Some("tabler-s1/3"));
        assert_eq!(rows[0].duration_ms, Some(13));
        assert_eq!(
            rows[0].started_at.as_deref(),
            Some("2023-11-14T22:13:20+00:00")
        );
        assert!(rows[0].own && !rows[0].current);
        assert!(rows[1].current && rows[1].own);
        assert_eq!(rows[1].state.as_deref(), Some("running (not cancellable)"));
        assert_eq!(rows[2].id, "n2:9");
        assert!(!rows[2].own);
        let run: Vec<&str> = of_run(&tasks, "tabler-s1/3")
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(run, ["n1:464"]);
        // Grouped by node (the API's default).
        let grouped = parse(
            r#"{"nodes":{"n1":{"name":"es","tasks":{"n1:7":{"node":"n1","id":7,"action":"a","cancellable":true}}}}}"#,
        )
        .unwrap();
        assert_eq!(parse_tasks(&grouped)[0].id, "n1:7");
        assert!(parse_tasks(&parse("[1]").unwrap()).is_empty());
    }

    #[test]
    fn cancel_replies() {
        assert!(kill_outcome("n:1", 200, br#"{"nodes":{}}"#).ok);
        let e = kill_outcome(
            "n:1",
            200,
            br#"{"node_failures":[{"type":"failed_node_exception","reason":"node gone"}]}"#,
        );
        assert_eq!((e.ok, e.message.as_deref()), (false, Some("node gone")));
        let e = kill_outcome(
            "n:2",
            404,
            br#"{"error":{"type":"resource_not_found_exception","reason":"task [n:2] is not found"},"status":404}"#,
        );
        assert!(!e.ok && e.message.unwrap().contains("is not found"));
    }
}
