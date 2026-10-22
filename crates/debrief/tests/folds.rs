//! Merge folds in the log, at the App level: with folding on the log
//! lists the first-parent spine, each merge row a fold whose groups splice
//! in when it opens (loaded lazily: off the UI thread with an event
//! sender, inline without), over `common::bubbles_repo`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use common::{
    BUBBLES_GROUPS, BUBBLES_SHARED, BUBBLES_SPINE, JjEnv, bubbles_repo, graph_repo, have_jj, keys,
    press, w,
};
use crossterm::event::KeyCode;
use debrief::app::{App, AppEvent, Group, Screen, StartOptions, is_fold_more};
use debrief::seen::Status;
use debrief::vcs;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    bubbles_repo(&root);
    Fixture {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    }
}

impl Fixture {
    fn opts(&self) -> StartOptions {
        let mut o = StartOptions::new(self.cache.clone());
        o.config.view.stats = false;
        o
    }

    fn app_with(&self, opts: StartOptions) -> App {
        open(&self.root, opts)
    }

    fn app(&self) -> App {
        self.app_with(self.opts())
    }
}

fn open(root: &Path, opts: StartOptions) -> App {
    App::new(vcs::open(root, vcs::OpenOptions::default()).unwrap(), opts).unwrap()
}

fn summaries(app: &App) -> Vec<String> {
    app.rows().iter().map(|r| r.info.summary.clone()).collect()
}

fn row(app: &App, summary: &str) -> usize {
    app.rows()
        .iter()
        .position(|r| r.info.summary == summary)
        .unwrap_or_else(|| panic!("no row {summary}: {:?}", summaries(app)))
}

fn id(app: &App, summary: &str) -> String {
    app.rows()[row(app, summary)].info.commit.clone()
}

fn open_fold(app: &mut App, summary: &str) {
    let i = row(app, summary);
    app.fold_open(i);
}

fn cursor_summary(app: &App) -> &str {
    &app.rows()[app.cursor()].info.summary
}

/// Feed the app the next `n` events the workers send: the count the step
/// is known to cause. Each has 20s to arrive.
#[track_caller]
fn settle_events(app: &mut App, rx: &mpsc::Receiver<AppEvent>, n: usize) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    for i in 1..=n {
        let ev = rx
            .recv_timeout(Duration::from_secs(20))
            .unwrap_or_else(|e| panic!("event {i} of {n}: {e}"));
        kinds.push(match &ev {
            AppEvent::FoldLoaded { .. } => "FoldLoaded",
            AppEvent::FoldPage { .. } => "FoldPage",
            AppEvent::MergeSizes { .. } => "MergeSizes",
            _ => "other",
        });
        app.event(ev);
    }
    kinds
}

#[test]
fn the_log_lists_the_spine_with_merges_as_closed_folds_and_their_sizes() {
    let f = fixture();
    let app = f.app();
    assert!(app.folding());
    assert_eq!(summaries(&app), BUBBLES_SPINE);
    let sizes = [
        ("Octopus x y z", 3),
        ("Merge n", 4),
        ("Merge b", 2),
        ("Merge a", 2),
    ];
    for r in app.rows() {
        assert_eq!(r.depth, 0);
        assert_eq!(
            (r.group, r.groups.is_empty(), &r.merge),
            (None, true, &None)
        );
        let want = sizes.iter().find(|(s, _)| *s == r.info.summary);
        match want {
            Some((_, n)) => {
                let fold = r.fold.unwrap();
                assert!(!fold.open, "{}", r.info.summary);
                assert_eq!(fold.size, Some(*n), "{}", r.info.summary);
            }
            None => assert_eq!(r.fold, None, "{}", r.info.summary),
        }
    }
    assert_eq!(app.bubble(0), None, "not loaded");
}

#[test]
fn opening_the_octopus_splices_three_marked_groups() {
    let f = fixture();
    let mut app = f.app();
    let octopus = id(&app, "Octopus x y z");
    app.fold_open(0);
    assert_eq!(
        summaries(&app)[..5],
        ["Octopus x y z", "x1", "y1", "z1", "Merge n"]
    );
    assert!(app.rows()[0].fold.unwrap().open);
    let marks: Vec<_> = app.rows()[1..4].iter().map(|r| r.group).collect();
    assert_eq!(
        marks,
        [Some(Group::Mid), Some(Group::Mid), Some(Group::Last)]
    );
    for r in &app.rows()[1..4] {
        assert_eq!(r.depth, 1);
        assert_eq!(r.groups, [r.group.unwrap()]);
        assert_eq!(r.merge.as_deref(), Some(octopus.as_str()));
        assert_eq!(r.fold, None);
    }
    let bubble = app.bubble(0).unwrap();
    let got: Vec<_> = bubble.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(got, ["x1", "y1", "z1"]);
}

#[test]
fn a_nested_merge_is_a_closed_fold_that_opens_one_level_deeper() {
    let f = fixture();
    let mut app = f.app();
    open_fold(&mut app, "Merge n");
    let groups = BUBBLES_GROUPS
        .iter()
        .find(|(m, _)| *m == "Merge n")
        .unwrap()
        .1;
    let at = row(&app, "Merge n");
    assert_eq!(summaries(&app)[at + 1..at + 4], *groups[0]);
    let s = &app.rows()[at + 1];
    assert_eq!(
        (s.depth, s.group),
        (1, None),
        "a 2-parent fold has no marker"
    );
    assert_eq!(s.fold.map(|f| (f.open, f.size)), Some((false, Some(1))));
    // Merge n's bubble: its own group, Merge s's not loaded yet.
    assert_eq!(app.bubble(at).unwrap().len(), 3);
    open_fold(&mut app, "Merge s");
    assert_eq!(
        summaries(&app)[at..at + 5],
        ["Merge n", "Merge s", "s1", "n2", "n1"]
    );
    let s1 = &app.rows()[at + 2];
    assert_eq!(s1.depth, 2);
    assert_eq!(s1.merge.as_deref(), Some(id(&app, "Merge s").as_str()));
    assert_eq!(app.bubble(at).unwrap().len(), 4, "nested where loaded");
}

#[test]
fn closing_from_inside_a_fold_moves_the_cursor_to_its_merge() {
    let f = fixture();
    let mut app = f.app();
    open_fold(&mut app, "Merge n");
    open_fold(&mut app, "Merge s");
    let i = row(&app, "s1");
    app.fold_close(i);
    assert_eq!(cursor_summary(&app), "Merge s");
    assert!(!app.rows()[app.cursor()].fold.unwrap().open);
    let i = row(&app, "n2");
    app.fold_close(i);
    assert_eq!(cursor_summary(&app), "Merge n");
    assert_eq!(summaries(&app), BUBBLES_SPINE);
    // Reopened, Merge s is still loaded and remembered closed.
    open_fold(&mut app, "Merge n");
    assert!(!app.rows()[row(&app, "Merge s")].fold.unwrap().open);
}

#[test]
fn open_folds_survive_a_reload() {
    let f = fixture();
    let mut app = f.app();
    open_fold(&mut app, "Octopus x y z");
    open_fold(&mut app, "Merge n");
    app.goto_summary("n2");
    let before = summaries(&app);
    app.reload(false);
    assert_eq!(summaries(&app), before);
    assert_eq!(cursor_summary(&app), "n2");
}

#[test]
fn open_and_close_all() {
    let f = fixture();
    let mut app = f.app();
    app.folds_open_all();
    let all: Vec<String> = summaries(&app);
    assert_eq!(all.len(), BUBBLES_SPINE.len() + 11, "{all:?}");
    assert!(all.contains(&"s1".to_string()), "nested opened too");
    app.goto_summary("s1");
    app.folds_close_all();
    assert_eq!(summaries(&app), BUBBLES_SPINE);
    assert_eq!(cursor_summary(&app), "Merge n", "the outermost merge");
}

#[test]
fn turning_folding_off_lists_flat_and_keeps_the_cursor_commit() {
    let f = fixture();
    let mut app = f.app();
    open_fold(&mut app, "Merge b");
    app.goto_summary("b1");
    app.set_folding(false);
    assert!(!app.folding());
    let flat = vcs::open(&f.root, vcs::OpenOptions::default())
        .unwrap()
        .log(None, 200)
        .unwrap();
    let want: Vec<_> = flat.iter().map(|c| c.summary.clone()).collect();
    assert_eq!(summaries(&app), want);
    assert!(app.rows().iter().all(|r| r.fold.is_none() && r.depth == 0));
    assert_eq!(cursor_summary(&app), "b1");
    // Back on, on a side commit: the nearest spine row above it in the
    // flat list (the merge that landed it); every fold closed.
    app.set_folding(true);
    assert_eq!(summaries(&app), BUBBLES_SPINE);
    assert_eq!(cursor_summary(&app), "Merge b");
    assert!(app.rows().iter().all(|r| r.fold.is_none_or(|f| !f.open)));
}

#[test]
fn fold_merges_off_starts_flat() {
    let f = fixture();
    let mut opts = f.opts();
    opts.config = debrief::app::config::Config::parse("[view]\nfold_merges = false\n").unwrap();
    let app = f.app_with(opts);
    assert!(!app.folding());
    assert_eq!(app.rows().len(), BUBBLES_SPINE.len() + 11);
    assert!(app.rows().iter().all(|r| r.fold.is_none()));
}

#[test]
fn set_folding_on_the_commit_screen_keeps_the_open_commit() {
    let f = fixture();
    let mut app = f.app();
    app.goto_summary("Merge n");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.screen(), Screen::Commit(_)));
    let open = app.commit_id().unwrap().to_string();
    app.set_folding(false);
    let Screen::Commit(i) = app.screen() else {
        panic!("closed the view")
    };
    assert_eq!(app.rows()[i].info.commit, open);
    assert_eq!(app.commit_id(), Some(open.as_str()));
}

#[test]
fn a_filter_hit_inside_an_open_fold_keeps_its_merges() {
    let f = fixture();
    let mut app = f.app();
    open_fold(&mut app, "Merge n");
    open_fold(&mut app, "Merge s");
    keys(&mut app, "fs1");
    press(&mut app, KeyCode::Enter);
    assert_eq!(summaries(&app), ["Merge n", "Merge s", "s1"]);
    // An unloaded fold is not searched (`x` is never in a hex ID).
    keys(&mut app, "f");
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "fx1");
    press(&mut app, KeyCode::Enter);
    assert!(summaries(&app).is_empty(), "{:?}", summaries(&app));
}

#[test]
fn a_full_group_ends_with_a_more_row_that_loads_the_next_page() {
    let f = fixture();
    let mut opts = f.opts();
    opts.limit = 2;
    let mut app = f.app_with(opts);
    assert_eq!(summaries(&app), ["Octopus x y z", "Merge n"]);
    open_fold(&mut app, "Merge n");
    let merge = id(&app, "Merge n");
    assert_eq!(
        summaries(&app),
        ["Octopus x y z", "Merge n", "Merge s", "n2", "… more"]
    );
    let more = app.rows()[4].clone();
    assert!(is_fold_more(&more.info.commit), "{}", more.info.commit);
    assert_eq!((more.depth, more.group, more.fold), (1, None, None));
    assert_eq!((more.status, more.working), (Status::Seen, false));
    assert_eq!(more.merge.as_deref(), Some(merge.as_str()));
    // A filter never matches it, and hides it without a hit in its group.
    keys(&mut app, "fmore");
    assert!(summaries(&app).is_empty(), "{:?}", summaries(&app));
    press(&mut app, KeyCode::Esc);
    // m and w skip it; Enter on it loads the group's next page and opens
    // nothing.
    app.goto_summary("… more");
    keys(&mut app, "m");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Log);
    assert_eq!(
        summaries(&app),
        ["Octopus x y z", "Merge n", "Merge s", "n2", "n1"]
    );
    assert_eq!(cursor_summary(&app), "n1", "the first new commit");
}

#[test]
fn a_commit_two_groups_list_shows_once_in_the_first() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    // y1 builds on x1: the octopus's y1 group reaches x1 too.
    graph_repo(
        &root,
        &[
            ("base", vec![], vec![w("b.txt", "b\n")]),
            ("x1", vec![1], vec![w("x.txt", "x\n")]),
            ("y1", vec![2], vec![w("y.txt", "y\n")]),
            ("z1", vec![1], vec![w("z.txt", "z\n")]),
            ("Octopus", vec![1, 2, 3, 4], vec![w("x.txt", "x\n")]),
        ],
    );
    let mut app = open(&root, StartOptions::new(tmp.path().join("cache")));
    assert_eq!(summaries(&app), ["Octopus", "base"]);
    app.fold_open(0);
    assert_eq!(summaries(&app), ["Octopus", "x1", "y1", "z1", "base"]);
    let marks: Vec<_> = app.rows()[1..4].iter().map(|r| r.group).collect();
    assert_eq!(
        marks,
        [Some(Group::Mid), Some(Group::Mid), Some(Group::Last)]
    );
}

#[test]
fn m_and_w_act_on_loaded_fold_commits_split_by_the_spine() {
    let f = fixture();
    let mut app = f.app();
    // The first run marked the spine seen; the bubbles are new.
    open_fold(&mut app, "Octopus x y z");
    assert_eq!(app.rows()[row(&app, "x1")].status, Status::New);
    keys(&mut app, "m");
    assert_eq!(app.rows()[row(&app, "x1")].status, Status::Seen);
    // w inside Merge n's fold splits at Merge n: the octopus and its
    // bubble are newer, Merge n's bubble goes with Merge n.
    open_fold(&mut app, "Merge n");
    let i = row(&app, "n2");
    app.set_watermark(i);
    let status = |s| app.rows()[row(&app, s)].status;
    assert_eq!(status("Octopus x y z"), Status::New);
    assert_eq!(status("y1"), Status::New);
    assert_eq!(status("Merge n"), Status::Seen);
    assert_eq!(status("Merge s"), Status::Seen);
    assert_eq!(status("n1"), Status::Seen);
}

#[test]
fn a_path_filter_narrows_the_spine_and_the_fold() {
    let f = fixture();
    let mut opts = f.opts();
    opts.path = Some(BUBBLES_SHARED.to_string());
    let mut app = f.app_with(opts);
    assert_eq!(summaries(&app), ["m1", "Merge b", "Merge a"]);
    open_fold(&mut app, "Merge a");
    assert_eq!(summaries(&app), ["m1", "Merge b", "Merge a", "a2"]);
}

#[test]
fn with_a_sender_folds_and_sizes_load_off_the_ui_thread() {
    let f = fixture();
    let mut app = f.app();
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    app.fold_open(0);
    assert_eq!(
        summaries(&app),
        BUBBLES_SPINE,
        "nothing spliced before the event"
    );
    assert!(app.rows()[0].fold.unwrap().open);
    assert_eq!(settle_events(&mut app, &rx, 1), ["FoldLoaded"]);
    assert_eq!(summaries(&app)[1..4], ["x1", "y1", "z1"]);
    // Merge n's fold lists Merge s, whose size then loads.
    open_fold(&mut app, "Merge n");
    assert_eq!(
        settle_events(&mut app, &rx, 2),
        ["FoldLoaded", "MergeSizes"]
    );
    let s = &app.rows()[row(&app, "Merge s")];
    assert_eq!(s.fold.unwrap().size, Some(1));
    // A reload re-reads nothing loaded: no event.
    app.reload(false);
    assert!(rx.try_recv().is_err());
}

#[test]
fn a_group_page_arrives_as_a_fold_page_event() {
    let f = fixture();
    let mut opts = f.opts();
    opts.limit = 2;
    let mut app = f.app_with(opts);
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    open_fold(&mut app, "Merge n");
    assert_eq!(
        settle_events(&mut app, &rx, 2),
        ["FoldLoaded", "MergeSizes"]
    );
    let i = row(&app, "… more");
    app.fold_open(i);
    assert_eq!(settle_events(&mut app, &rx, 1), ["FoldPage"]);
    assert_eq!(summaries(&app)[2..], ["Merge s", "n2", "n1"]);
}

#[test]
fn a_stale_fold_loaded_leaves_the_live_request_pending() {
    let f = fixture();
    let mut app = f.app();
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    app.fold_open(0);
    let merge = app.rows()[0].info.commit.clone();
    // An answer to an older request (0: requests count from 1) is dropped
    // and the live one still lands.
    app.event(AppEvent::FoldLoaded {
        request: 0,
        merge,
        result: Ok(Vec::new()),
    });
    assert_eq!(settle_events(&mut app, &rx, 1), ["FoldLoaded"]);
    assert!(app.rows()[0].fold.unwrap().open);
    assert_eq!(summaries(&app)[1..4], ["x1", "y1", "z1"]);
}

#[test]
fn a_stale_fold_page_leaves_the_live_request_pending() {
    let f = fixture();
    let mut opts = f.opts();
    opts.limit = 2;
    let mut app = f.app_with(opts);
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    open_fold(&mut app, "Merge n");
    assert_eq!(
        settle_events(&mut app, &rx, 2),
        ["FoldLoaded", "MergeSizes"]
    );
    let merge = id(&app, "Merge n");
    let parent = app.rows()[row(&app, "Merge n")].info.parents[1].clone();
    let i = row(&app, "… more");
    app.fold_open(i);
    app.event(AppEvent::FoldPage {
        request: 0,
        merge,
        parent,
        result: Ok(Vec::new()),
    });
    assert_eq!(settle_events(&mut app, &rx, 1), ["FoldPage"]);
    assert_eq!(summaries(&app)[2..], ["Merge s", "n2", "n1"]);
}

/// jj: no working-row event re-triggers the fold load, so the fold
/// reloads under the new path only if the requery's own request survives
/// the stale answer to the first.
#[test]
fn a_path_requery_during_a_fold_load_ends_with_the_fold_loaded() {
    if !have_jj() {
        return;
    }
    let env = JjEnv::over_git(bubbles_repo);
    let tmp = tempfile::tempdir().unwrap();
    let mut opts = StartOptions::new(tmp.path().join("cache"));
    opts.config.view.stats = false;
    let mut app = open(&env.root, opts);
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    open_fold(&mut app, "Merge a");
    // The load's answer stays queued while the path changes.
    keys(&mut app, &format!("F{BUBBLES_SHARED}"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(summaries(&app), ["m1", "Merge b", "Merge a"]);
    // The first load (stale), the requery's reload of the open fold.
    assert_eq!(
        settle_events(&mut app, &rx, 2),
        ["FoldLoaded", "FoldLoaded"]
    );
    let a = &app.rows()[row(&app, "Merge a")];
    assert!(a.fold.unwrap().open);
    assert_eq!(summaries(&app), ["m1", "Merge b", "Merge a", "a2"]);
}

#[test]
fn a_revset_with_several_heads_is_listed_flat_without_folds() {
    if !have_jj() {
        return;
    }
    let env = JjEnv::over_git(bubbles_repo);
    let v = vcs::open(&env.root, vcs::OpenOptions::default()).unwrap();
    let by = common::bubbles_by_summary(&*v, "::@- ~ root()");
    let tmp = tempfile::tempdir().unwrap();
    let mut opts = StartOptions::new(tmp.path().join("cache"));
    opts.revset = Some(format!("::({} | {})", by["x1"].commit, by["y1"].commit));
    let app = App::new(v, opts).unwrap();
    assert!(app.folding());
    assert!(
        app.rows().len() > BUBBLES_SPINE.len(),
        "{:?}",
        summaries(&app)
    );
    assert!(app.rows().iter().all(|r| r.fold.is_none()));
    assert!(
        app.status().contains("merges flat: several heads"),
        "{}",
        app.status()
    );
}

#[test]
fn a_one_head_revset_that_skips_spine_commits_keeps_its_folds() {
    if !have_jj() {
        return;
    }
    let env = JjEnv::over_git(bubbles_repo);
    let tmp = tempfile::tempdir().unwrap();
    let mut opts = StartOptions::new(tmp.path().join("cache"));
    opts.config.view.stats = false;
    opts.revset = Some(r#"::@- ~ root() ~ description(exact:"m1\n")"#.to_string());
    let app = open(&env.root, opts);
    let want: Vec<&str> = BUBBLES_SPINE
        .iter()
        .copied()
        .filter(|s| *s != "m1")
        .collect();
    assert_eq!(summaries(&app), want);
    assert!(app.rows()[0].fold.is_some(), "the octopus folds");
    assert_eq!(app.status(), "");
}

trait GotoSummary {
    fn goto_summary(&mut self, s: &str);
}

impl GotoSummary for App {
    /// Move the log cursor to the row `s` with `j` / `k`.
    fn goto_summary(&mut self, s: &str) {
        let to = row(self, s);
        while self.cursor() < to {
            keys(self, "j");
        }
        while self.cursor() > to {
            keys(self, "k");
        }
    }
}
