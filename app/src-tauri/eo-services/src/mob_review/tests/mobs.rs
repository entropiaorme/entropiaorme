//! The review a line for each mob, the kills the screen could not name,
//! and the labels mobs are shown by.

use super::*;
use crate::mob_display::{self, LabelSource};

impl Rig {
    fn mobs(&self, session: &str) -> Vec<ReviewedMob> {
        self.runtime
            .block_on(self.review.mobs(session))
            .unwrap()
            .mobs
    }

    fn settle_mob(
        &self,
        session: &str,
        mob: &ReviewedRef,
        target: ReviewTarget,
    ) -> Result<Settled, MobReviewError> {
        match self
            .runtime
            .block_on(self.review.settle_mob(session, mob.clone(), target))?
        {
            SettleOutcome::Settled(settled) => Ok(settled),
            SettleOutcome::DidYouMean(creature) => panic!("offered {creature:?} instead"),
        }
    }

    /// Two kills of `session` the screen answered without naming anything.
    fn unnamed_kills(&self) {
        self.execute(
            "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
             mob_stamp_source, timestamp, cost_ped, loot_total_ped) VALUES \
             ('u1', 's1', 'Unknown', '', '', NULL, 1700.0, 0.5, 0.4), \
             ('u2', 's1', 'Unknown', '', '', NULL, 1701.0, 0.5, 0.4); \
             INSERT INTO kill_mob_traces (kill_id, session_id, screen, continuity, reads) VALUES \
             ('u1', 's1', 'indistinct', 'mixed', '[]'), \
             ('u2', 's1', 'undecided', 'disagree', '[]');",
        );
    }
}

fn mob(species: &str, maturity: &str, key: Option<&str>) -> ReviewedRef {
    ReviewedRef::Mob {
        species: species.into(),
        maturity: maturity.into(),
        provisional_key: key.map(str::to_string),
    }
}

/// A line as the tests compare it: the mob shown, its kills, how many the
/// screen was less than sure of, what they are recorded as, and whether it
/// is on no list and settled.
type Line = (Option<String>, i64, i64, Option<String>, bool, bool);

fn lines(mobs: &[ReviewedMob]) -> Vec<Line> {
    mobs.iter()
        .map(|m| {
            (
                m.name.clone(),
                m.kill_count,
                m.unsure_count,
                m.current_name.clone(),
                m.unlisted,
                m.reviewed,
            )
        })
        .collect()
}

#[test]
fn each_mob_is_one_line_however_sure_the_screen_was_of_each_kill() {
    let rig = rig();
    hunt_with_continuity(&rig);
    // Gloomrat: two kills read and one carried on. Atrox Old: one read
    // clearly, two carried on. Daikiba Mature: a suggestion its kill did not
    // take. The mob on no list asks first; the pinned kill is not listed.
    let mobs = rig.mobs("s1");
    assert_eq!(
        lines(&mobs),
        [
            (some("Gloomrat"), 3, 1, some("Gloomrat"), true, false),
            (some("Atrox Old"), 3, 2, some("Atrox Old"), false, false),
            (some("Daikiba Mature"), 1, 1, some("Unknown"), false, false),
        ]
    );
    assert_eq!(mobs[0].mob, mob("Gloomrat", "", Some("GLOOMRAT L4 #0")));
    assert_eq!(mobs[1].mob, mob("Atrox", "Old", None));
    assert_eq!(mobs[1].example_text, some("Atrox Old"));
    // The same kills, by grade, are seven groups' worth of answers in five
    // groups: the line is what the player reads.
    assert_eq!(rig.groups("s1").len(), 5);
}

#[test]
fn a_line_settles_every_kill_of_its_mob_in_one_action() {
    let rig = rig();
    hunt_with_continuity(&rig);
    // The three Atrox Old kills, read clearly or carried on, go back to the
    // declared mob together.
    rig.settle_mob("s1", &mob("Atrox", "Old", None), ReviewTarget::Declared)
        .unwrap();
    for kill in ["k3", "k5", "k6"] {
        assert_eq!(rig.stamp(kill).0, "Atrox Young", "{kill}");
    }
    // The suggestion is taken by naming its line.
    let settled = rig
        .settle_mob(
            "s1",
            &mob("Daikiba", "Mature", None),
            ReviewTarget::Creature("Daikiba Mature".into()),
        )
        .unwrap();
    assert!(matches!(settled, Settled::Listed(_)));
    assert_eq!(rig.stamp("k4").0, "Daikiba Mature");
    assert_eq!(
        lines(&rig.mobs("s1")),
        [
            (some("Gloomrat"), 3, 1, some("Gloomrat"), true, false),
            (some("Atrox Old"), 3, 2, some("Atrox Young"), false, true),
            (
                some("Daikiba Mature"),
                1,
                1,
                some("Daikiba Mature"),
                false,
                true
            ),
        ]
    );
    // A mob no kill of the session was given is no line.
    assert!(matches!(
        rig.settle_mob("s1", &mob("Atrox", "Young", None), ReviewTarget::Declared),
        Err(MobReviewError::NotFound(_))
    ));
}

#[test]
fn the_kills_the_screen_could_not_name_are_one_line_named_in_one_action_and_put_back() {
    let rig = rig();
    hunt(&rig);
    // Every kill was given a mob: there is no such line, and nothing to name.
    assert!(rig.mobs("s1").iter().all(|m| m.mob != ReviewedRef::Unnamed));
    assert!(matches!(
        rig.settle_mob("s1", &ReviewedRef::Unnamed, ReviewTarget::Declared),
        Err(MobReviewError::NotFound(_))
    ));
    rig.unnamed_kills();
    let line = |rig: &Rig| rig.mobs("s1").last().cloned().expect("a line");
    let unnamed = line(&rig);
    assert_eq!(unnamed.mob, ReviewedRef::Unnamed);
    assert_eq!(
        (unnamed.name.clone(), unnamed.kill_count, unnamed.reviewed),
        (None, 2, false)
    );
    assert_eq!(unnamed.current_name, None);
    // Named: both kills are the mob typed, by the player's word.
    rig.settle_mob(
        "s1",
        &ReviewedRef::Unnamed,
        ReviewTarget::Creature("Atrox Old".into()),
    )
    .unwrap();
    for kill in ["u1", "u2"] {
        assert_eq!(
            rig.stamp(kill),
            (
                "Atrox Old".into(),
                "Atrox".into(),
                "Old".into(),
                some("declared"),
                None
            )
        );
    }
    let named = line(&rig);
    assert_eq!(
        (named.current_name.clone(), named.kill_count, named.reviewed),
        (some("Atrox Old"), 2, true)
    );
    // Named again as something else, the stamp kept from before is still
    // the one they had at first.
    rig.settle_mob(
        "s1",
        &ReviewedRef::Unnamed,
        ReviewTarget::Creature("Daikiba Mature".into()),
    )
    .unwrap();
    assert_eq!(rig.stamp("u1").0, "Daikiba Mature");
    // Put back: no mob again, and the line asks as before.
    rig.settle_mob("s1", &ReviewedRef::Unnamed, ReviewTarget::Declared)
        .unwrap();
    for kill in ["u1", "u2"] {
        assert_eq!(
            rig.stamp(kill),
            ("Unknown".into(), String::new(), String::new(), None, None)
        );
    }
    let back = line(&rig);
    assert_eq!((back.current_name.clone(), back.reviewed), (None, false));
    assert_eq!(
        rig.scalar::<i64>("SELECT COUNT(*) FROM kill_mob_assigned"),
        2
    );
}

#[test]
fn the_kills_not_named_cannot_be_named_while_the_session_runs() {
    let rig = rig();
    rig.session("s1", true);
    rig.unnamed_kills();
    assert!(matches!(
        rig.settle_mob(
            "s1",
            &ReviewedRef::Unnamed,
            ReviewTarget::Creature("Atrox Old".into())
        ),
        Err(MobReviewError::Conflict(_))
    ));
    assert_eq!(rig.stamp("u1").0, "Unknown");
}

#[test]
fn a_label_changes_what_a_mob_is_shown_as_everywhere_and_nothing_about_which_mob_it_is() {
    let rig = rig();
    hunt_with_continuity(&rig);
    let labels = rig
        .runtime
        .block_on(rig.review.set_label("Atrox Old", " Old   Trox "))
        .unwrap();
    assert_eq!(
        labels,
        [KeptLabel {
            species: "Atrox".into(),
            maturity: "Old".into(),
            printed: "Atrox Old".into(),
            label: "Old Trox".into(),
            by_player: true,
        }]
    );
    // Every kill of the mob reads so, the mob itself unchanged; the kept
    // answers follow, so the review still gathers them as one line.
    for kill in ["k3", "k5", "k6"] {
        let stamp = rig.stamp(kill);
        assert_eq!(
            (stamp.0.as_str(), stamp.1.as_str(), stamp.2.as_str()),
            ("Old Trox", "Atrox", "Old"),
            "{kill}"
        );
    }
    let mobs = rig.mobs("s1");
    assert_eq!(
        (
            mobs[1].name.clone(),
            mobs[1].kill_count,
            mobs[1].current_name.clone()
        ),
        (some("Old Trox"), 3, some("Old Trox"))
    );
    // What the tracker names kills by is in step, and a kill settled as the
    // mob from now on takes the label.
    assert_eq!(rig.labels.chosen("Atrox", "Old"), some("Old Trox"));
    rig.settle_mob(
        "s1",
        &mob("Daikiba", "Mature", None),
        ReviewTarget::Creature("Atrox Old".into()),
    )
    .unwrap();
    assert_eq!(rig.stamp("k4").0, "Old Trox");
    // The mob is found by its label too, to change it.
    let labels = rig
        .runtime
        .block_on(rig.review.set_label("old trox", "Trox"))
        .unwrap();
    assert_eq!(labels.len(), 1);
    assert_eq!(labels[0].label, "Trox");
    assert_eq!(rig.stamp("k3").0, "Trox");
    // Removed, the mob is shown as the game prints it again.
    let labels = rig
        .runtime
        .block_on(rig.review.clear_label("Atrox", "Old"))
        .unwrap();
    assert!(labels.is_empty());
    assert_eq!(rig.stamp("k3").0, "Atrox Old");
    assert_eq!(rig.stamp("k4").0, "Atrox Old");
    assert_eq!(rig.labels.chosen("Atrox", "Old"), None);
}

#[test]
fn a_label_is_held_to_what_a_label_can_be_and_given_only_to_a_mob_the_reader_knows() {
    let rig = rig();
    let set = |mob: &str, label: &str| rig.runtime.block_on(rig.review.set_label(mob, label));
    for (mob, label) in [
        ("Atrox Old", "   "),
        ("Atrox Old", "Old\u{200b}Trox"),
        ("Gloomrat Bruiser", "Bruiser"),
        ("", "Trox"),
    ] {
        assert!(
            matches!(set(mob, label), Err(MobReviewError::Invalid(_))),
            "{mob:?} as {label:?}"
        );
    }
    assert!(matches!(
        set("Atrox Old", &"x".repeat(65)),
        Err(MobReviewError::Invalid(_))
    ));
    assert!(rig
        .runtime
        .block_on(rig.review.labels())
        .unwrap()
        .is_empty());
}

#[test]
fn a_label_read_off_a_plate_gives_way_to_the_players_and_never_replaces_it() {
    let rig = rig();
    rig.session("s1", false);
    let write = |label: &'static str, source: LabelSource| {
        rig.runtime
            .block_on(rig.db.with_writer(move |conn| {
                let tx = conn.transaction()?;
                let changed = mob_display::set_label(&tx, "Atrox", "4", label, source, 5.0)?;
                tx.commit()?;
                Ok(changed)
            }))
            .unwrap()
    };
    assert!(write("Atrox 04", LabelSource::Plate));
    // Read again, nothing changes.
    assert!(!write("Atrox 04", LabelSource::Plate));
    let listed = rig.runtime.block_on(rig.review.labels()).unwrap();
    assert_eq!(
        (listed[0].label.as_str(), listed[0].by_player),
        ("Atrox 04", false)
    );
    assert!(write("Trox Four", LabelSource::Player));
    assert!(!write("Atrox 04", LabelSource::Plate));
    let listed = rig.runtime.block_on(rig.review.labels()).unwrap();
    assert_eq!(
        (listed[0].label.as_str(), listed[0].by_player),
        ("Trox Four", true)
    );
}
