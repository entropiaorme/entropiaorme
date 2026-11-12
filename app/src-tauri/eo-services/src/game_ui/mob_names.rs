//! Resolving a creature's name read off its nameplate.
//!
//! A single reading of a plate's name is unreliable: the recogniser
//! misses a character or two of a serif name drawn over the scene more
//! often than not. So a reading is matched against the creatures the
//! application knows (the bundled creature list, and names the player
//! has confirmed), and the match says how it was made: spelled whole, or
//! a near miss. A near miss is a guess; deciding on a creature is left to
//! votes over many readings (see [`super::mob_history`]).
//!
//! The client prints a creature's name and its maturity on one line, but
//! in which order is not established, so both orders are known for every
//! creature with maturities, each meaning the same creature; a creature
//! without maturities is known by its name alone. The bundled list tells
//! some creatures of one name apart by the planet, in brackets after the
//! name, which the client has no need to print; such a name is also known
//! without its bracketed part.
//!
//! A creature the list does not know is a normal event (the game adds
//! creatures), and its name must not be forced onto its nearest
//! neighbour: a reading is a known creature only when it comes close to
//! that one creature and no other. The level printed beside the name,
//! when it reads, decides between creatures equally close and vetoes a
//! near miss whose maturity has another level.

use serde::Deserialize;
use serde_json::Value;

use crate::fuzzy_match::{ratio, PERFECT};
use crate::game_data_store::GameDataStore;

use super::field::{key_counts, label_key, similarity_bound, too_few_letters, KeyCounts};

/// How strictly a reading must match a known name, as the profile
/// describes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobNameProfile {
    /// Least similarity (0 to 100) at which a reading is taken as a
    /// known name strictly: as near as a misreading of a character or two
    /// comes, and rarely an unlisted creature does.
    pub known_score: f64,
    /// Least similarity at which a reading is taken as a known name at
    /// all: below the strict floor it is a loose match, which can only
    /// ever support a guess.
    pub guess_score: f64,
    /// Names within this of the best similarity are all in contention:
    /// a reading is one creature only when no other is in contention.
    pub margin: f64,
    /// A reading whose level is not the level of any creature in
    /// contention is taken as one of them only at this similarity or
    /// above; below it, it is a creature not on the list.
    pub sure_score: f64,
    /// A reading with fewer letters than this, or with letters making
    /// less than this share of its characters, is not a name.
    pub min_letters: usize,
    pub min_letter_share: f64,
    /// Maturity names the bundled list uses as placeholders where a
    /// creature has no real maturity: such a creature is also known by its
    /// name alone.
    pub placeholder_maturities: Vec<String>,
}

/// One creature the bundled list knows, at one maturity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueMob {
    pub name: String,
    pub species: Option<String>,
    pub maturity: Option<String>,
    /// The level the list gives this maturity, when it gives one.
    pub level: Option<u32>,
}

/// Who a key names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Who {
    Mob(usize),
    Confirmed(usize),
}

struct Key {
    chars: Vec<char>,
    counts: KeyCounts,
    who: Who,
}

impl Key {
    fn new(text: &str, who: Who) -> Option<Self> {
        let chars = label_key(text);
        (!chars.is_empty()).then(|| Key {
            counts: key_counts(&chars),
            chars,
            who,
        })
    }
}

/// The creatures a reading is matched against: the bundled list, and
/// names the player has confirmed for creatures it lacks.
///
/// A confirmed name that spells a listed creature, or comes within the
/// loose band of one, is dropped: it would shadow or blur that creature.
/// The caller must not confirm a provisional name that is a misspelling of
/// a listed creature as it stands; it is that listed creature, misread.
#[derive(Default)]
pub struct MobCatalogue {
    mobs: Vec<CatalogueMob>,
    confirmed: Vec<String>,
    keys: Vec<Key>,
}

/// A name without the bracketed part the bundled list adds to tell
/// creatures of one name apart: `Atrox (Calypso)` is `Atrox`.
fn without_qualifier(name: &str) -> Option<&str> {
    let trimmed = name.trim_end();
    if !trimmed.ends_with(')') {
        return None;
    }
    let open = trimmed.rfind('(')?;
    let stem = trimmed[..open].trim_end();
    (!stem.is_empty()).then_some(stem)
}

/// The ways the client may print a creature on its plate: its name and
/// maturity in either order, or its name alone when it has no maturity;
/// and each again without the bracketed part of a name.
pub fn plate_strings(name: &str, maturity: Option<&str>, profile: &MobNameProfile) -> Vec<String> {
    let placeholder = maturity.is_some_and(|m| {
        profile
            .placeholder_maturities
            .iter()
            .any(|p| p.eq_ignore_ascii_case(m))
    });
    let mut names = vec![name];
    names.extend(without_qualifier(name));
    names
        .into_iter()
        .flat_map(|name| {
            let mut plates = match maturity {
                Some(maturity) => vec![format!("{name} {maturity}"), format!("{maturity} {name}")],
                None => vec![name.to_string()],
            };
            if placeholder {
                plates.push(name.to_string());
            }
            plates
        })
        .collect()
}

impl MobCatalogue {
    /// A catalogue of `mobs`, and of the names in `confirmed` (names the
    /// player has confirmed for creatures the list does not know).
    pub fn new(
        mobs: impl IntoIterator<Item = CatalogueMob>,
        confirmed: impl IntoIterator<Item = String>,
        profile: &MobNameProfile,
    ) -> Self {
        let mut catalogue = MobCatalogue::default();
        for mob in mobs {
            let index = catalogue.mobs.len();
            catalogue.keys.extend(
                plate_strings(&mob.name, mob.maturity.as_deref(), profile)
                    .iter()
                    .filter_map(|p| Key::new(p, Who::Mob(index))),
            );
            catalogue.mobs.push(mob);
        }
        for name in confirmed {
            let key = label_key(&name);
            let shadows = key.is_empty()
                || catalogue
                    .keys
                    .iter()
                    .any(|k| ratio(&key, &k.chars) >= profile.guess_score);
            if shadows {
                continue;
            }
            let index = catalogue.confirmed.len();
            catalogue
                .keys
                .extend(Key::new(&name, Who::Confirmed(index)));
            catalogue.confirmed.push(name);
        }
        catalogue
    }

    /// From the bundled game data's creature entities: one creature per
    /// maturity (`name`, `species.name`, and each of `maturities`' `name`
    /// and `level`), or one without a maturity where the entity lists
    /// none.
    pub fn from_entities(
        entities: &[Value],
        confirmed: impl IntoIterator<Item = String>,
        profile: &MobNameProfile,
    ) -> Self {
        let mut mobs = Vec::new();
        for entity in entities {
            let Some(name) = entity
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|n| !n.is_empty())
            else {
                continue;
            };
            let species = entity
                .get("species")
                .and_then(|s| s.get("name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from);
            let maturities: Vec<(String, Option<u32>)> = entity
                .get("maturities")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|m| {
                            let maturity = m.get("name")?.as_str()?.trim();
                            let level = m
                                .get("level")
                                .and_then(Value::as_u64)
                                .and_then(|l| u32::try_from(l).ok());
                            (!maturity.is_empty()).then(|| (maturity.to_string(), level))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if maturities.is_empty() {
                mobs.push(CatalogueMob {
                    name: name.to_string(),
                    species: species.clone(),
                    maturity: None,
                    level: None,
                });
            }
            for (maturity, level) in maturities {
                mobs.push(CatalogueMob {
                    name: name.to_string(),
                    species: species.clone(),
                    maturity: Some(maturity),
                    level,
                });
            }
        }
        Self::new(mobs, confirmed, profile)
    }

    /// From the bundled game data's `mobs` collection.
    pub fn from_store(
        store: &GameDataStore,
        confirmed: impl IntoIterator<Item = String>,
        profile: &MobNameProfile,
    ) -> Self {
        Self::from_entities(store.get_entities("mobs"), confirmed, profile)
    }

    pub fn mobs(&self) -> &[CatalogueMob] {
        &self.mobs
    }
}

/// How a reading matched a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spelling {
    /// It spelled the name whole, up to case, spacing and punctuation.
    Exact,
    /// It came close to the name, at the strict floor, and to no other.
    Near,
    /// It came close to the name and to no other, but only above the
    /// lower floor: a creature the list lacks comes this close to one it
    /// has as often as a misreading does.
    Loose,
}

/// What a name read off a plate is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// A creature on the bundled list, as the list names it.
    Known {
        name: String,
        species: Option<String>,
        maturity: Option<String>,
        spelling: Spelling,
    },
    /// A name the player has confirmed.
    Confirmed { name: String, spelling: Spelling },
    /// Creatures the bundled list tells apart only by the planet in
    /// brackets after their names, alike in species, maturity and level,
    /// whose plates print the same: the plate cannot say which of them it
    /// is, only this species at this maturity, as the plate names it.
    Species {
        name: String,
        species: String,
        maturity: Option<String>,
        spelling: Spelling,
    },
    /// Clear text near no known name: a creature the application does
    /// not know, or a reading too far off to say which it is. With the
    /// level read beside it, when it read.
    New { text: String, level: Option<u32> },
    /// Text too short or too noisy to be a name, or as close to two
    /// creatures as to each other.
    Unclear,
}

/// Which creature a candidate is: a creature's two orders, or two
/// entries of one maturity, are one.
fn identity(catalogue: &MobCatalogue, who: Who) -> (u8, &str, Option<&str>) {
    match who {
        Who::Mob(i) => {
            let mob = &catalogue.mobs[i];
            (0, mob.name.as_str(), mob.maturity.as_deref())
        }
        Who::Confirmed(i) => (1, catalogue.confirmed[i].as_str(), None),
    }
}

/// The candidates, each creature once at its best score.
fn distinct(catalogue: &MobCatalogue, candidates: &[(Who, f64)]) -> Vec<(Who, f64)> {
    let mut out: Vec<(Who, f64)> = Vec::new();
    for &(who, score) in candidates {
        let id = identity(catalogue, who);
        match out.iter_mut().find(|(w, _)| identity(catalogue, *w) == id) {
            Some(seen) if seen.1 < score => *seen = (who, score),
            Some(_) => {}
            None => out.push((who, score)),
        }
    }
    out
}

fn spelling_of(score: f64, profile: &MobNameProfile) -> Spelling {
    if score >= PERFECT {
        Spelling::Exact
    } else if score >= profile.known_score {
        Spelling::Near
    } else {
        Spelling::Loose
    }
}

fn answer(catalogue: &MobCatalogue, who: Who, score: f64, profile: &MobNameProfile) -> Resolved {
    let spelling = spelling_of(score, profile);
    match who {
        Who::Mob(i) => {
            let mob = &catalogue.mobs[i];
            Resolved::Known {
                name: mob.name.clone(),
                species: mob.species.clone(),
                maturity: mob.maturity.clone(),
                spelling,
            }
        }
        Who::Confirmed(i) => Resolved::Confirmed {
            name: catalogue.confirmed[i].clone(),
            spelling,
        },
    }
}

/// Creatures the bundled list tells apart only by the planet in brackets
/// after their names, at one maturity of one species, print alike on their
/// plates: the reading names that species at that maturity, by the name
/// the plate prints ([`Resolved::Species`]), not any one of them, whatever
/// their levels (the level never picks a planet). Anything else among
/// `tied` is not one creature.
fn printed_alike(
    catalogue: &MobCatalogue,
    tied: &[(Who, f64)],
    profile: &MobNameProfile,
) -> Option<Resolved> {
    let mobs: Vec<&CatalogueMob> = tied
        .iter()
        .map(|(who, _)| match who {
            Who::Mob(i) => Some(&catalogue.mobs[*i]),
            Who::Confirmed(_) => None,
        })
        .collect::<Option<_>>()?;
    let first = mobs.first()?;
    let printed = without_qualifier(&first.name).unwrap_or(&first.name);
    let alike = mobs.iter().all(|m| {
        without_qualifier(&m.name).unwrap_or(&m.name) == printed
            && m.maturity == first.maturity
            && m.species.is_some()
            && m.species == first.species
    });
    alike.then(|| Resolved::Species {
        name: printed.to_string(),
        species: first.species.clone().unwrap_or_default(),
        maturity: first.maturity.clone(),
        spelling: spelling_of(tied[0].1, profile),
    })
}

/// Resolve `text`, a name read off a plate whose level read as `level`
/// (`None` when it did not read), against `catalogue`.
///
/// A name the player confirmed and the reading spells whole is that name.
/// Otherwise every known name is compared with the reading; if none comes
/// within the profile's similarity the reading is [`Resolved::New`]. A
/// reading is a known creature only when one creature stands alone at the
/// best similarity, with no other within the profile's margin below it.
///
/// The level is never what names a creature: the bundled list's levels
/// are not proven to be the ones plates print. It only breaks a tie among
/// creatures at the best similarity that do not print alike, and refuses: a creature whose level
/// is not the one read is unclear when another maturity of the same name
/// has the level read (a maturity's name can be read off another's plate,
/// `Old` off `Old Alpha`), and is taken as a creature not on the list when
/// the reading is not all but exact. A confirmed name has no level, so the
/// level neither supports nor refuses it.
pub fn resolve(
    text: &str,
    level: Option<u32>,
    catalogue: &MobCatalogue,
    profile: &MobNameProfile,
) -> Resolved {
    let new = || Resolved::New {
        text: text.trim().to_string(),
        level,
    };
    if too_few_letters(text, profile.min_letters, profile.min_letter_share) {
        return Resolved::Unclear;
    }
    let key = label_key(text);
    if let Some(i) = catalogue
        .confirmed
        .iter()
        .position(|name| label_key(name) == key)
    {
        return answer(catalogue, Who::Confirmed(i), PERFECT, profile);
    }
    let read_counts = key_counts(&key);
    let floor = profile.guess_score - profile.margin;
    let candidates: Vec<(Who, f64)> = catalogue
        .keys
        .iter()
        .filter(|k| similarity_bound(&k.counts, k.chars.len(), &read_counts, key.len()) >= floor)
        .filter_map(|k| {
            let score = ratio(&key, &k.chars);
            (score >= floor).then_some((k.who, score))
        })
        .collect();
    let best = candidates.iter().map(|c| c.1).fold(f64::MIN, f64::max);
    if candidates.is_empty() || best < profile.guess_score {
        return new();
    }
    let creatures = distinct(catalogue, &candidates);
    let top: Vec<(Who, f64)> = creatures.iter().copied().filter(|c| c.1 >= best).collect();
    if creatures
        .iter()
        .any(|c| c.1 < best && c.1 >= best - profile.margin)
    {
        return Resolved::Unclear;
    }
    let level_of = |who: Who| match who {
        Who::Mob(i) => catalogue.mobs[i].level,
        Who::Confirmed(_) => None,
    };
    // By the name the plate prints, so a maturity of a creature the list
    // tells apart by planet counts for every one of them.
    let printed = |m: &CatalogueMob| without_qualifier(&m.name).unwrap_or(&m.name).to_string();
    // A level read that is none of `tied`'s but another maturity's of the
    // same name: the reading may be that maturity's name cut short.
    let another_maturity = |tied: &[(Who, f64)], read: u32| {
        let mobs: Vec<&CatalogueMob> = tied
            .iter()
            .filter_map(|(who, _)| match who {
                Who::Mob(i) => Some(&catalogue.mobs[*i]),
                Who::Confirmed(_) => None,
            })
            .collect();
        !mobs.iter().any(|m| m.level == Some(read))
            && catalogue.mobs.iter().any(|m| {
                m.level == Some(read)
                    && mobs
                        .iter()
                        .any(|t| printed(m) == printed(t) && m.maturity != t.maturity)
            })
    };
    if top.len() > 1 {
        if let Some(species) = printed_alike(catalogue, &top, profile) {
            return match level {
                Some(read) if another_maturity(&top, read) => Resolved::Unclear,
                _ => species,
            };
        }
    }
    let chosen = match (top.as_slice(), level) {
        ([single], _) => *single,
        (_, Some(read)) if top.iter().all(|(who, _)| matches!(who, Who::Mob(_))) => {
            let agreeing: Vec<(Who, f64)> = top
                .iter()
                .copied()
                .filter(|(who, _)| level_of(*who) == Some(read))
                .collect();
            return match agreeing.as_slice() {
                [single] => answer(catalogue, single.0, single.1, profile),
                [] => Resolved::Unclear,
                _ => printed_alike(catalogue, &agreeing, profile).unwrap_or(Resolved::Unclear),
            };
        }
        _ => return printed_alike(catalogue, &top, profile).unwrap_or(Resolved::Unclear),
    };
    let (who, score) = chosen;
    if let (Some(read), Who::Mob(i)) = (level, who) {
        let mob = &catalogue.mobs[i];
        if mob.level.is_some_and(|l| l != read) {
            if another_maturity(&[(who, score)], read) {
                return Resolved::Unclear;
            }
            if score < profile.sure_score {
                return new();
            }
        }
    }
    answer(catalogue, who, score, profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_ui::test_support::{bundled_mobs, bundled_profile};

    fn names() -> MobNameProfile {
        bundled_profile().mob_names
    }

    fn bundled(confirmed: Vec<String>) -> MobCatalogue {
        MobCatalogue::from_entities(&bundled_mobs(), confirmed, &names())
    }

    fn of(mobs: Vec<CatalogueMob>, confirmed: Vec<String>) -> MobCatalogue {
        MobCatalogue::new(mobs, confirmed, &names())
    }

    fn mob(name: &str, species: &str, maturity: Option<&str>, level: Option<u32>) -> CatalogueMob {
        CatalogueMob {
            name: name.into(),
            species: Some(species.into()),
            maturity: maturity.map(String::from),
            level,
        }
    }

    fn known(
        name: &str,
        maturity: Option<&str>,
        spelling: Spelling,
    ) -> Option<(String, Option<String>, Spelling)> {
        Some((name.into(), maturity.map(String::from), spelling))
    }

    /// The creature a resolution names, with how it was spelled.
    fn named(resolved: &Resolved) -> Option<(String, Option<String>, Spelling)> {
        match resolved {
            Resolved::Known {
                name,
                maturity,
                spelling,
                ..
            } => Some((name.clone(), maturity.clone(), *spelling)),
            _ => None,
        }
    }

    fn new(text: &str, level: Option<u32>) -> Resolved {
        Resolved::New {
            text: text.into(),
            level,
        }
    }

    #[test]
    fn a_creature_is_known_by_its_name_and_maturity_in_either_order() {
        let catalogue = bundled(Vec::new());
        let want = known("Combibo", Some("Young"), Spelling::Exact);
        for text in [
            "Combibo Young",
            "Young Combibo",
            "combibo  young.",
            "CombiboYoung",
        ] {
            for level in [Some(3), None] {
                let got = resolve(text, level, &catalogue, &names());
                assert_eq!(named(&got), want, "{text:?} {level:?}");
            }
        }
        match resolve("Combibo Young", Some(3), &catalogue, &names()) {
            Resolved::Known { species, .. } => assert_eq!(species.as_deref(), Some("Combibo")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_reading_a_character_or_two_off_is_a_near_miss_of_the_one_creature_near_it() {
        let catalogue = bundled(Vec::new());
        for text in ["Combibo Yuong", "Cornbibo Young", "Combib0 Young"] {
            assert_eq!(
                named(&resolve(text, Some(3), &catalogue, &names())),
                known("Combibo", Some("Young"), Spelling::Near),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_creature_the_list_does_not_know_is_new_however_it_reads() {
        let catalogue = bundled(Vec::new());
        for (text, level) in [
            ("Zombie Fleshripper", Some(25)),
            ("Zombie Fleshripper", None),
            ("ZombieFleshriper", Some(25)),
            ("Zombie Fleshrippe", None),
        ] {
            assert_eq!(
                resolve(text, level, &catalogue, &names()),
                new(text, level),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_near_miss_whose_level_disagrees_is_not_forced_onto_its_neighbour() {
        // A creature the list lacks whose name is one letter from one it
        // has: the level refuses the near miss; without a level it is only
        // ever a near miss, a guess, never an exact match.
        let catalogue = of(
            vec![mob("Snarg", "Snarg", Some("Old"), Some(14))],
            Vec::new(),
        );
        assert_eq!(
            resolve("Snarg 0ld", Some(9), &catalogue, &names()),
            new("Snarg 0ld", Some(9))
        );
        assert_eq!(
            named(&resolve("Snarg 0ld", Some(14), &catalogue, &names())),
            known("Snarg", Some("Old"), Spelling::Near)
        );
        assert_eq!(
            named(&resolve("Snarg 0ld", None, &catalogue, &names())),
            known("Snarg", Some("Old"), Spelling::Near)
        );
        // An exact name whose level is another than the list's, with no
        // other maturity at that level: the level may be what misread.
        assert_eq!(
            named(&resolve("Snarg Old", Some(9), &catalogue, &names())),
            known("Snarg", Some("Old"), Spelling::Exact)
        );
    }

    #[test]
    fn a_level_belonging_to_another_maturity_of_the_name_makes_the_reading_unclear() {
        // `Old` read off an `Old Alpha` plate, and `Gen 07` off a `Gen 01`
        // plate, each with the level the plate printed.
        let catalogue = of(
            vec![
                mob("Snarg", "Snarg", Some("Old"), Some(14)),
                mob("Snarg", "Snarg", Some("Old Alpha"), Some(19)),
                mob("Drone", "Drone", Some("Gen 01"), Some(10)),
                mob("Drone", "Drone", Some("Gen 07"), Some(40)),
            ],
            Vec::new(),
        );
        assert_eq!(
            resolve("Snarg Old", Some(19), &catalogue, &names()),
            Resolved::Unclear
        );
        assert_eq!(
            resolve("Drone Gen 07", Some(10), &catalogue, &names()),
            Resolved::Unclear
        );
        // With their own levels, or none, they read as themselves.
        assert_eq!(
            named(&resolve("Snarg Old Alpha", Some(19), &catalogue, &names())),
            known("Snarg", Some("Old Alpha"), Spelling::Exact)
        );
        assert_eq!(
            named(&resolve("Drone Gen 07", None, &catalogue, &names())),
            known("Drone", Some("Gen 07"), Spelling::Exact)
        );
        // A reading as near one as the other is decided by the level, and
        // unclear without it.
        assert_eq!(
            named(&resolve("Drone Gen 0l", Some(10), &catalogue, &names())),
            known("Drone", Some("Gen 01"), Spelling::Near)
        );
        assert_eq!(
            resolve("Drone Gen 0l", None, &catalogue, &names()),
            Resolved::Unclear
        );
    }

    #[test]
    fn the_level_never_lifts_a_weaker_reading_over_the_best() {
        // `Kerbo Olde` is nearer `Kerbo Olden` than `Kerbo Old`, but within
        // the margin of both: a rival that close leaves it unclear, and the
        // level of either does not choose it.
        let catalogue = of(
            vec![
                mob("Kerbo", "Kerbo", Some("Old"), Some(5)),
                mob("Kerbo", "Kerbo", Some("Olden"), Some(7)),
            ],
            Vec::new(),
        );
        for level in [Some(7), Some(5), None] {
            assert_eq!(
                resolve("Kerbo Olde", level, &catalogue, &names()),
                Resolved::Unclear,
                "{level:?}"
            );
        }
        // Equally near two maturities: the level breaks the tie.
        let catalogue = of(
            vec![
                mob("Kerbo", "Kerbo", Some("Old"), Some(5)),
                mob("Kerbo", "Kerbo", Some("Odd"), Some(7)),
            ],
            Vec::new(),
        );
        assert_eq!(
            named(&resolve("Kerbo Oxd", Some(7), &catalogue, &names())),
            known("Kerbo", Some("Odd"), Spelling::Near)
        );
        assert_eq!(
            resolve("Kerbo Oxd", None, &catalogue, &names()),
            Resolved::Unclear
        );
    }

    #[test]
    fn creatures_printed_alike_are_named_by_their_shared_species_whatever_the_level() {
        let catalogue = of(
            vec![
                mob("Atrox (Calypso)", "Atrox", Some("Young"), Some(4)),
                mob("Atrox (Rocktropia)", "Atrox", Some("Young"), Some(4)),
                mob("Atrox (Calypso)", "Atrox", Some("Old"), Some(6)),
                mob("Atrox (Rocktropia)", "Atrox", Some("Old"), Some(9)),
                mob("Pandi Pup", "Pandi", None, None),
                mob("Pandi", "Pandi", Some("Pup"), Some(2)),
            ],
            Vec::new(),
        );
        // Alike in every way the plate shows: the species at that maturity.
        for level in [Some(4), None] {
            assert_eq!(
                resolve("Atrox Young", level, &catalogue, &names()),
                Resolved::Species {
                    name: "Atrox".into(),
                    species: "Atrox".into(),
                    maturity: Some("Young".into()),
                    spelling: Spelling::Exact,
                },
                "{level:?}"
            );
        }
        // Levels that differ do not pick a planet: the level a plate prints
        // is not proven to be the list's.
        for level in [Some(9), Some(6), Some(7), None] {
            assert_eq!(
                resolve("Atrox Old", level, &catalogue, &names()),
                Resolved::Species {
                    name: "Atrox".into(),
                    species: "Atrox".into(),
                    maturity: Some("Old".into()),
                    spelling: Spelling::Exact,
                },
                "{level:?}"
            );
        }
        // Another maturity's level still refuses: `Old` may be `Old Alpha`
        // cut short.
        let mut with_alpha = catalogue.mobs.clone();
        with_alpha.push(mob("Atrox (Calypso)", "Atrox", Some("Old Alpha"), Some(11)));
        let with_alpha = of(with_alpha, Vec::new());
        assert_eq!(
            resolve("Atrox Old", Some(11), &with_alpha, &names()),
            Resolved::Unclear
        );
        assert_eq!(
            named(&resolve(
                "Atrox (Calypso) Young",
                None,
                &catalogue,
                &names()
            )),
            known("Atrox (Calypso)", Some("Young"), Spelling::Exact)
        );
        // Two creatures whose plates print alike but which are not one
        // species at one maturity.
        assert_eq!(
            named(&resolve("Pandi Pup", Some(2), &catalogue, &names())),
            known("Pandi", Some("Pup"), Spelling::Exact)
        );
        assert_eq!(
            resolve("Pandi Pup", None, &catalogue, &names()),
            Resolved::Unclear
        );
    }

    #[test]
    fn the_bundled_itumatrox_pair_is_one_species_at_each_maturity_whatever_the_level() {
        // The two planets' `Old` and `Mature` swap levels (17 and 16) on
        // the list: the level read never picks between them.
        let catalogue = MobCatalogue::from_entities(&bundled_mobs(), Vec::new(), &names());
        for (text, maturity) in [("Itumatrox Old", "Old"), ("Itumatrox Mature", "Mature")] {
            for level in [Some(16), Some(17), None] {
                assert_eq!(
                    resolve(text, level, &catalogue, &names()),
                    Resolved::Species {
                        name: "Itumatrox".into(),
                        species: "Atrox".into(),
                        maturity: Some(maturity.into()),
                        spelling: Spelling::Exact,
                    },
                    "{text} at {level:?}"
                );
            }
        }
    }

    #[test]
    fn a_maturity_of_a_creature_told_apart_by_planet_refuses_a_level_for_all_of_them() {
        // `Old` read off a Rocktropia `Old Alpha`'s plate, with its level:
        // the Calypso `Old` must not take it.
        let catalogue = of(
            vec![
                mob("Atrox (Calypso)", "Atrox", Some("Old"), Some(6)),
                mob("Atrox (Rocktropia)", "Atrox", Some("Old Alpha"), Some(11)),
            ],
            Vec::new(),
        );
        assert_eq!(
            resolve("Atrox (Calypso) Old", Some(11), &catalogue, &names()),
            Resolved::Unclear
        );
    }

    #[test]
    fn a_confirmed_name_that_would_shadow_or_blur_a_listed_creature_is_dropped() {
        let catalogue = of(
            vec![mob("Combibo", "Combibo", Some("Young"), Some(3))],
            vec![
                "Combibo Young".into(),
                "Combibo Yuong".into(),
                "Zombie Fleshripper".into(),
            ],
        );
        assert_eq!(catalogue.confirmed, vec!["Zombie Fleshripper".to_string()]);
        assert_eq!(
            named(&resolve("Combibo Yuong", Some(3), &catalogue, &names())),
            known("Combibo", Some("Young"), Spelling::Near)
        );
    }

    #[test]
    fn a_placeholder_maturity_is_known_by_the_name_alone_as_well() {
        let catalogue = of(
            vec![mob("Turrelion", "Turrelion", Some("Default"), Some(30))],
            Vec::new(),
        );
        for text in ["Turrelion", "Turrelion Default", "Default Turrelion"] {
            assert_eq!(
                named(&resolve(text, Some(30), &catalogue, &names())),
                known("Turrelion", Some("Default"), Spelling::Exact),
                "{text:?}"
            );
        }
    }

    #[test]
    fn confirmed_names_match_first_whatever_the_level_and_near_misses_of_them_are_guesses() {
        let catalogue = bundled(vec!["Zombie Fleshripper".to_string()]);
        for level in [Some(25), Some(3), None] {
            assert_eq!(
                resolve("zombie fleshripper", level, &catalogue, &names()),
                Resolved::Confirmed {
                    name: "Zombie Fleshripper".into(),
                    spelling: Spelling::Exact
                },
                "{level:?}"
            );
            assert_eq!(
                resolve("Zombie Fleshriper", level, &catalogue, &names()),
                Resolved::Confirmed {
                    name: "Zombie Fleshripper".into(),
                    spelling: Spelling::Near
                },
                "{level:?}"
            );
        }
    }

    #[test]
    fn text_too_short_or_too_noisy_is_unclear() {
        let catalogue = bundled(Vec::new());
        for text in ["", "  ", "Zo", "...", "#%&*!x", "1.25", "L25"] {
            assert_eq!(
                resolve(text, None, &catalogue, &names()),
                Resolved::Unclear,
                "{text:?}"
            );
        }
    }

    #[test]
    fn the_bundled_list_gives_every_maturity_and_bare_names_where_it_has_none() {
        let entities: Vec<Value> = serde_json::from_str(
            r#"[
                {"name": "Combibo", "species": {"name": "Combibo"},
                 "maturities": [{"name": "Young", "level": 3}, {"name": " ", "level": 9}, {"name": "Old"}]},
                {"name": "Foul", "maturities": []},
                {"name": "  "},
                {"species": {"name": "Nameless"}}
            ]"#,
        )
        .expect("json");
        let catalogue = MobCatalogue::from_entities(&entities, Vec::new(), &names());
        assert_eq!(
            catalogue.mobs(),
            &[
                mob("Combibo", "Combibo", Some("Young"), Some(3)),
                mob("Combibo", "Combibo", Some("Old"), None),
                CatalogueMob {
                    name: "Foul".into(),
                    species: None,
                    maturity: None,
                    level: None
                },
            ]
        );
        assert_eq!(
            named(&resolve("Foul", Some(30), &catalogue, &names())),
            known("Foul", None, Spelling::Exact)
        );
    }

    #[test]
    fn creatures_left_out_of_the_list_are_rarely_taken_for_another_and_never_exactly() {
        // Each of a sample of the bundled creatures, left out of the list
        // and read whole with its own level. Spelled whole as another it
        // would be named as that creature: never. At the strict floor or in
        // the loose band it can only be a guess; otherwise it is new or
        // unclear.
        let entities = bundled_mobs();
        let (mut exact, mut strict, mut loose, mut new, mut unclear) = (0, 0, 0, 0, 0);
        for (i, left_out) in entities.iter().enumerate().step_by(9) {
            let rest: Vec<Value> = entities
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, e)| e.clone())
                .collect();
            let catalogue = MobCatalogue::from_entities(&rest, Vec::new(), &names());
            let alone =
                MobCatalogue::from_entities(std::slice::from_ref(left_out), Vec::new(), &names());
            for mob in alone.mobs().iter().take(2) {
                let text = match &mob.maturity {
                    Some(m) => format!("{} {m}", mob.name),
                    None => mob.name.clone(),
                };
                // A few creatures print exactly as another does (`Thawr
                // Divine` is a creature, and `Thawr` at maturity `Divine`):
                // no reading can tell those apart, so they are not tried.
                if catalogue.keys.iter().any(|k| k.chars == label_key(&text)) {
                    continue;
                }
                match resolve(&text, mob.level, &catalogue, &names()) {
                    Resolved::Known { spelling, .. } | Resolved::Species { spelling, .. } => {
                        match spelling {
                            Spelling::Exact => exact += 1,
                            Spelling::Near => strict += 1,
                            Spelling::Loose => loose += 1,
                        }
                    }
                    Resolved::New { .. } => new += 1,
                    Resolved::Unclear => unclear += 1,
                    Resolved::Confirmed { .. } => panic!("no names are confirmed"),
                }
            }
        }
        let tried = exact + strict + loose + new + unclear;
        println!(
            "creatures left out of the list, of {tried}: {exact} spelled whole as another, \
             {strict} near another at the strict floor, {loose} near another only in the loose \
             band, {new} new, {unclear} unclear"
        );
        assert!(tried >= 100, "{tried}");
        assert_eq!(exact, 0);
        assert!(
            (strict + loose) * 10 <= tried,
            "{strict} + {loose} of {tried}"
        );
    }
}
