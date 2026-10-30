mod common;

use common::{App, Instance};
use serde_json::{json, Value};

fn names(list: &Value) -> Vec<String> {
    list["panes"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap_or("-").to_string()).collect()
}

#[test]
fn with_no_app_every_pane_type_opens_and_an_app_shows_it_later() {
    let instance = Instance::start();
    instance.open("sh", Some("cat"));
    for (cmd, args) in [
        ("open", json!({"type": "web", "name": "site", "url": "localhost:1", "history": true})),
        ("open", json!({"type": "md", "name": "doc", "path": "/tmp/a.md"})),
        ("open", json!({"type": "code", "name": "src", "path": "/tmp/a.rs", "line": 3})),
        ("open", json!({"type": "ios", "name": "phone", "app": "com.example.App", "wait": false})),
        ("restart", json!({"pane": "doc"})),
    ] {
        let reply = instance.call(cmd, args.clone());
        assert_eq!(reply["ok"], true, "{cmd} {args}: {reply}");
    }
    let navigated = instance.call("navigate", json!({"pane": "site", "url": "localhost:3"}));
    assert_eq!(navigated["pane"]["url"], "http://localhost:3", "{navigated}");
    let back = instance.call("navigate", json!({"pane": "site", "back": true}));
    assert_eq!(back["pane"]["url"], "http://localhost:1", "{back}");
    let marked = instance.call("markup", json!({"pane": "doc", "agent": "sh"}));
    assert_eq!((marked["pane"]["markup"].clone(), marked["pane"]["agent"].clone()), (json!(true), json!("term-1")), "{marked}");
    let nowhere = instance.call("markup", json!({"pane": "doc", "agent": "nobody"}));
    assert_eq!(nowhere["error"]["code"], "not_found", "{nowhere}");
    let plain = instance.call("markup", json!({"pane": "doc", "off": true}));
    assert!(plain["pane"]["markup"].is_null(), "{plain}");
    let wrong = instance.call("markup", json!({"pane": "sh"}));
    assert_eq!(wrong["error"]["code"], "wrong_type", "{wrong}");
    assert_eq!(instance.call("debug.web", json!({"pane": "site"}))["error"]["code"], "no_app");
    assert_eq!(instance.call("comment", json!({"path": "/tmp/a.rs", "line": 3, "text": "why"}))["error"]["code"], "no_app");
    for cmd in ["send", "read"] {
        let reply = instance.call(cmd, json!({"pane": "site", "text": "x"}));
        assert_eq!(reply["error"]["code"], "wrong_type", "{cmd}: {reply}");
    }
    let wrong = instance.call("navigate", json!({"pane": "sh", "url": "example.com"}));
    assert_eq!(wrong["error"]["code"], "wrong_type", "{wrong}");
    let list = instance.call("list", json!({}));
    assert_eq!(names(&list), vec!["sh", "site", "doc", "src", "phone"], "{list}");
    let capabilities = instance.call("capabilities", json!({}));
    assert_eq!(capabilities["paneTypes"], json!(["term", "web", "ios", "md", "code", "diff"]), "{capabilities}");

    let mut app = App::connect(&instance.socket());
    assert!(app.view.to_string().contains("\"page-1\""), "{}", app.view);
    assert_eq!(instance.call("close", json!({"pane": "site"}))["ok"], true);
    let stop = app.until("stop", |m| m["cmd"] == "stop");
    assert_eq!(stop["args"]["pane"], "page-1", "{stop}");
}

#[test]
fn updates_from_the_app_change_the_record_and_come_back_to_it() {
    let instance = Instance::start();
    instance.call("open", json!({"type": "web", "name": "site", "url": "http://a.test/", "history": true}));
    let mut app = App::connect(&instance.socket());
    app.tell("update", json!({"pane": "page-1", "url": "http://b.test/"}));
    let echoed = app.until("the record", |m| m["cmd"] == "pane");
    assert_eq!(echoed["args"]["pane"]["url"], "http://b.test/", "{echoed}");
    app.tell("update", json!({"pane": "page-1", "title": "Page B"}));
    let titled = app.until("the titled record", |m| m["cmd"] == "pane");
    assert_eq!((titled["args"]["pane"]["url"].clone(), titled["args"]["pane"]["title"].clone()), (json!("http://b.test/"), json!("Page B")), "{titled}");
    app.tell("update", json!({"pane": "page-1", "title": "\u{1b}[31mRed\u{7}"}));
    let cleaned = app.until("the cleaned record", |m| m["cmd"] == "pane");
    assert_eq!(cleaned["args"]["pane"]["title"], "[31mRed", "{cleaned}");
    app.tell("update", json!({"pane": "page-1", "title": ""}));
    let untitled = app.until("the untitled record", |m| m["cmd"] == "pane");
    assert!(untitled["args"]["pane"]["title"].is_null(), "{untitled}");
    app.send(json!({"id": 5, "cmd": "navigate", "args": {"pane": "site", "back": true}}));
    let stop = app.until("stop", |m| m["cmd"] == "stop");
    assert_eq!(stop["args"]["pane"], "page-1");
    let start = app.until("start", |m| m["cmd"] == "start");
    assert_eq!(start["args"]["pane"]["url"], "http://a.test/", "{start}");
    let reply = app.until("the reply", |m| m["id"] == 5);
    assert_eq!(reply["pane"]["url"], "http://a.test/", "{reply}");
    assert_eq!(reply["pane"]["state"], "running", "{reply}");
    app.tell("update", json!({"pane": "page-1", "state": "failed", "error": "gone"}));
    app.until("the record", |m| m["cmd"] == "pane");
    let _answering = app.answering();
    let site = instance.pane("site");
    assert_eq!((site["state"].clone(), site["error"].clone()), (json!("failed"), json!("gone")), "{site}");
}

#[test]
fn when_the_app_goes_away_its_panes_stay_and_a_waiting_open_finishes() {
    let instance = Instance::start();
    instance.open("sh", Some("cat"));
    let mut app = App::connect(&instance.socket());
    app.shows = false;
    let socket = instance.socket();
    let caller = std::thread::spawn(move || common::call(&socket, "open", json!({"type": "web", "name": "site", "url": "a.test"})));
    app.until("start", |m| m["cmd"] == "start");
    drop(app);
    let opened = caller.join().unwrap();
    assert_eq!(opened["pane"]["state"], "running", "{opened}");
    let list = instance.call("list", json!({}));
    assert_eq!(names(&list), vec!["sh", "site"], "{list}");
    assert_eq!(list["windows"][0]["tabs"][0]["layout"]["children"][1]["pane"], "site", "{list}");
}

#[test]
fn a_comment_opens_its_file_once_and_stays_on_the_pane() {
    let instance = Instance::start();
    let app = App::connect(&instance.socket());
    let _answering = app.answering();
    let first = instance.call("comment", json!({"path": "/tmp/tour.rs", "line": 5, "text": "Queues the message."}));
    assert_eq!((first["opened"].clone(), first["first"].clone(), first["last"].clone()), (json!(true), json!(5), json!(7)), "{first}");
    let id = first["pane"]["id"].as_str().unwrap().to_string();
    assert_eq!(first["pane"]["type"], "code", "{first}");
    assert_eq!(first["pane"]["line"], 5, "{first}");
    assert_eq!(first["pane"]["comment"], json!({"path": "/tmp/tour.rs", "first": 5, "last": 7, "text": "Queues the message."}), "{first}");

    let again = instance.call("comment", json!({"path": "/tmp/tour.rs", "line": 20, "last": 24, "text": "Sends it.", "window": "win-1"}));
    assert_eq!((again["opened"].clone(), again["pane"]["id"].clone()), (json!(false), json!(id)), "{again}");
    assert_eq!((again["first"].clone(), again["last"].clone(), again["pane"]["line"].clone()), (json!(20), json!(24), json!(20)), "{again}");
    assert_eq!(again["pane"]["comment"]["text"], "Sends it.", "{again}");

    let other = instance.call("comment", json!({"path": "/tmp/other.rs", "line": 1, "text": "The other side.", "window": "win-1"}));
    assert_eq!(other["opened"], true, "{other}");
    assert_ne!(other["pane"]["id"], json!(id), "{other}");
    let placed = instance.call("comment", json!({"path": "/tmp/tour.rs", "line": 1, "text": "Again.", "tab": true}));
    assert_eq!(placed["opened"], true, "{placed}");

    let list = instance.call("list", json!({}));
    let tour = list["panes"].as_array().unwrap().iter().find(|p| p["id"] == json!(id)).unwrap().clone();
    assert_eq!(tour["comment"]["first"], 20, "{list}");
    for (args, code) in [
        (json!({"path": "/tmp/tour.rs", "line": 5}), "bad_request"),
        (json!({"path": "/tmp/tour.rs", "line": 5, "text": "  "}), "bad_request"),
        (json!({"path": "/tmp/tour.rs", "line": 9, "last": 3, "text": "x"}), "bad_request"),
        (json!({"line": 5, "text": "x"}), "bad_request"),
    ] {
        let reply = instance.call("comment", args.clone());
        assert_eq!(reply["error"]["code"], code, "{args}: {reply}");
    }
}

#[test]
fn a_tour_steps_through_its_stops_without_the_agent() {
    let mut instance = Instance::start();
    let app = App::connect(&instance.socket());
    let _answering = app.answering();
    instance.open("agent", Some("cat"));
    let stops = json!([
        {"path": "/tmp/tour.rs", "line": 5, "text": "Queues it."},
        {"path": "/tmp/tour.rs", "line": 20, "last": 24, "text": "Sends it."},
        {"path": "/tmp/other.rs", "line": 1, "text": "Takes it."},
    ]);
    let none = instance.call("tour", json!({"action": "new", "stops": stops}));
    assert_eq!(none["error"]["code"], "bad_request", "{none}");
    let queued = instance.call("tour", json!({"action": "new", "from": "agent", "stops": stops}));
    assert_eq!(queued["tour"], json!({"at": 1, "of": 3}), "{queued}");
    assert_eq!((queued["first"].clone(), queued["last"].clone(), queued["opened"].clone()), (json!(5), json!(7), json!(true)), "{queued}");
    let first = queued["pane"]["id"].as_str().unwrap().to_string();
    let agent = instance.pane("agent")["id"].as_str().unwrap().to_string();
    assert_eq!(queued["pane"]["tour"], json!({"owner": agent, "at": 1, "of": 3}), "{queued}");

    let next = instance.call("tour", json!({"action": "go", "owner": agent, "to": "next"}));
    assert_eq!((next["pane"]["id"].clone(), next["first"].clone(), next["last"].clone()), (json!(first), json!(20), json!(24)), "{next}");
    assert_eq!(next["pane"]["tour"]["at"], 2, "{next}");
    let other = instance.call("tour", json!({"action": "go", "from": "agent", "to": "next"}));
    assert_ne!(other["pane"]["id"], json!(first), "{other}");
    let marks = |instance: &Instance| -> Vec<(String, Value)> {
        let list = instance.call("list", json!({}));
        list["panes"].as_array().unwrap().iter().filter(|p| p["type"] == "code").map(|p| (p["id"].as_str().unwrap().to_string(), p["tour"].clone())).collect()
    };
    let shown = marks(&instance);
    assert_eq!(shown.iter().filter(|(_, mark)| !mark.is_null()).count(), 1, "{shown:?}");
    assert!(shown.iter().any(|(id, mark)| id == &first && mark.is_null()), "{shown:?}");

    let asked = instance.call("tour", json!({"action": "add", "from": "agent", "next": true, "stops": [{"path": "/tmp/client.rs", "line": 9, "text": "An answer."}]}));
    assert_eq!(asked["tour"], json!({"at": 4, "of": 4}), "{asked}");
    let back = instance.call("tour", json!({"action": "go", "from": "agent", "to": "back"}));
    assert_eq!((back["tour"].clone(), back["path"].clone()), (json!({"at": 3, "of": 4}), json!("/tmp/other.rs")), "{back}");
    let later = instance.call("tour", json!({"action": "add", "from": "agent", "stops": [{"path": "/tmp/end.rs", "line": 2, "text": "Last."}]}));
    assert_eq!((later["tour"].clone(), later["added"].clone()), (json!({"at": 3, "of": 5}), json!(1)), "{later}");
    let listed = instance.call("tour", json!({"from": "agent"}));
    assert_eq!(listed["at"], 3, "{listed}");
    let paths: Vec<&str> = listed["stops"].as_array().unwrap().iter().map(|s| s["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["/tmp/tour.rs", "/tmp/tour.rs", "/tmp/other.rs", "/tmp/client.rs", "/tmp/end.rs"], "{listed}");
    let jumped = instance.call("tour", json!({"action": "go", "from": "agent", "to": 5}));
    assert_eq!(jumped["tour"], json!({"at": 5, "of": 5}), "{jumped}");
    assert_eq!(instance.screen("agent").trim(), "", "stepping sent the agent something");

    instance.restart(libc::SIGTERM);
    let app = App::connect(&instance.socket());
    let _answering = app.answering();
    common::wait_for("the agent back", || !instance.pane("agent").is_null());
    assert_eq!(instance.call("tour", json!({"from": "agent"}))["at"], 5);
    assert_eq!(marks(&instance).iter().filter(|(_, mark)| mark["at"] == 5).count(), 1);
    let done = instance.call("tour", json!({"action": "go", "from": "agent", "to": "next"}));
    assert_eq!((done["done"].clone(), done["of"].clone()), (json!(true), json!(5)), "{done}");
    assert!(marks(&instance).iter().all(|(_, mark)| mark.is_null()));
    assert_eq!(instance.call("tour", json!({"from": "agent"}))["stops"], json!([]));

    for (args, code) in [
        (json!({"action": "new", "from": "agent", "stops": []}), "bad_request"),
        (json!({"action": "new", "from": "agent", "stops": [{"path": "/tmp/a.rs", "line": 1, "text": "x"}, {"path": "/tmp/b.rs", "line": 9, "last": 3, "text": "x"}]}), "bad_request"),
        (json!({"action": "go", "from": "agent", "to": "next"}), "not_found"),
        (json!({"action": "go", "from": "nobody", "to": "next"}), "not_found"),
        (json!({"action": "fly", "from": "agent"}), "bad_request"),
    ] {
        let reply = instance.call("tour", args.clone());
        assert_eq!(reply["error"]["code"], code, "{args}: {reply}");
    }
    assert_eq!(instance.call("tour", json!({"from": "agent"}))["stops"], json!([]), "a refused tour was kept");
    let code = instance.call("list", json!({}))["panes"].as_array().unwrap().iter().find(|p| p["type"] == "code").unwrap()["id"].clone();
    let wrong = instance.call("tour", json!({"from": code}));
    assert_eq!(wrong["error"]["code"], "wrong_type", "{wrong}");

    instance.call("tour", json!({"action": "new", "from": "agent", "stops": stops}));
    instance.call("close", json!({"pane": "agent"}));
    assert!(marks(&instance).iter().all(|(_, mark)| mark.is_null()), "the closed agent's tour stayed");
}

#[test]
fn a_tour_stop_on_a_doc_is_the_agents_thread_written_once() {
    let instance = Instance::start();
    let app = App::connect(&instance.socket());
    let _answering = app.answering();
    instance.open("agent", Some("cat"));
    let dir = std::env::temp_dir().join(format!("kanna-tour-doc-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let doc = dir.join("trip.md");
    std::fs::write(&doc, "# Trip\n\nThe keeper queues\nthe message.\n").unwrap();
    let doc = doc.to_string_lossy().to_string();
    let past = instance.call("tour", json!({"action": "new", "from": "agent", "stops": [{"path": doc, "line": 9, "text": "Past."}]}));
    assert_eq!(past["error"]["code"], "bad_request", "{past}");
    let gone = instance.call("tour", json!({"action": "new", "from": "agent", "stops": [{"path": dir.join("gone.md"), "line": 1, "text": "Gone."}]}));
    assert_eq!(gone["error"]["code"], "not_found", "{gone}");
    let queued = instance.call("tour", json!({"action": "new", "from": "agent", "stops": [{"path": doc, "line": 4, "text": "The trip."}, {"path": "/tmp/tour.rs", "line": 5, "text": "Code."}]}));
    assert_eq!((queued["thread"].clone(), queued["first"].clone(), queued["opened"].clone()), (json!("c1"), json!(3), json!(true)), "{queued}");
    let agent = instance.pane("agent")["id"].clone();
    assert_eq!(queued["pane"]["type"], "md", "{queued}");
    assert_eq!((queued["pane"]["markup"].clone(), queued["pane"]["agent"].clone()), (json!(true), agent.clone()), "{queued}");
    assert_eq!(queued["pane"]["tour"], json!({"owner": agent, "at": 1, "of": 2, "thread": "c1"}), "{queued}");
    instance.call("tour", json!({"action": "go", "from": "agent", "to": "next"}));
    let back = instance.call("tour", json!({"action": "go", "from": "agent", "to": "back"}));
    assert_eq!((back["thread"].clone(), back["opened"].clone(), back["pane"]["id"].clone()), (json!("c1"), json!(false), queued["pane"]["id"].clone()), "{back}");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(".kanna/docs/trip.md.json")).unwrap()).unwrap();
    assert_eq!(saved["threads"].as_array().unwrap().len(), 1, "{saved}");
    assert_eq!(saved["threads"][0]["messages"][0]["text"], "The trip.", "{saved}");
    std::fs::remove_dir_all(&dir).unwrap();
}
