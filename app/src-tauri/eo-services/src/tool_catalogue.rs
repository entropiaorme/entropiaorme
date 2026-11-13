//! The tools a name read off the held-tool bar is matched against, as the
//! player's data stands: every library entry a player can hold, with
//! whether it is enabled, the names the player dismissed as not a tracked
//! tool, and the tools the bundled game data knows.
//!
//! The watcher matches against a catalogue held in a
//! [`ToolCatalogueCell`]. While the reading is on, whatever changes what
//! the catalogue is built from (an entry added, edited, deleted, turned on
//! or off; a name dismissed) rebuilds it and puts it in place, through
//! [`ToolCatalogues`]: the watcher's next look matches with the new one.
//! While the reading is off nothing is built.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::db::{Db, DbError};
use crate::game_data_store::GameDataStore;
use crate::game_ui::tool_names::{LibraryTool, ToolCatalogue, ToolKind};
use crate::tool_watch::ToolCatalogueCell;

/// A library row (id, name, item type, properties, enabled) as the
/// held-tool bar names it, when it is one a player can hold. Consumables
/// are never held, so never on the bar.
pub fn holdable(
    (id, name, item_type, _, enabled): &(i64, String, String, String, bool),
) -> Option<LibraryTool> {
    let kind = match item_type.as_str() {
        "weapon" => ToolKind::Weapon,
        "healing" => ToolKind::Healing,
        "tool" => ToolKind::Tool,
        _ => return None,
    };
    Some(LibraryTool {
        id: *id,
        name: name.clone(),
        kind,
        enabled: *enabled,
    })
}

/// The names the player has dismissed as not a tracked tool, oldest
/// first.
pub fn dismissed_names(conn: &rusqlite::Connection) -> Result<Vec<String>, DbError> {
    let mut stmt =
        conn.prepare("SELECT name FROM dismissed_tool_names ORDER BY dismissed_at, name")?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(names)
}

/// The player's data a catalogue is built from: the library's holdable
/// entries and the dismissed names.
async fn read_inputs(db: &Db) -> Result<(Vec<LibraryTool>, Vec<String>), DbError> {
    let rows = db.equipment_library_rows().await?;
    let dismissed = db.with_reader(|conn| dismissed_names(conn)).await?;
    Ok((rows.iter().filter_map(holdable).collect(), dismissed))
}

/// Why a catalogue could not be built.
#[derive(Debug, thiserror::Error)]
pub enum CatalogueError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("the catalogue build did not finish: {0}")]
    Build(#[from] tokio::task::JoinError),
}

/// The catalogue as the player's data stands now. Keying the bundled tools
/// is a few thousand names' work, so it runs off the async runtime.
pub async fn build_catalogue(
    db: &Db,
    store: &Arc<GameDataStore>,
) -> Result<ToolCatalogue, CatalogueError> {
    let (library, dismissed) = read_inputs(db).await?;
    let store = store.clone();
    Ok(
        tokio::task::spawn_blocking(move || ToolCatalogue::from_store(&store, library, dismissed))
            .await?,
    )
}

/// Whether reading the held tool is on, read live.
pub type ReadingOn = Arc<dyn Fn() -> bool + Send + Sync>;

/// The catalogue the held-tool watcher matches against, and what it is
/// rebuilt from.
///
/// It is built only while the reading is on: an edit made while it is off
/// is only noted, and the catalogue is built from the data as it then
/// stands when the reading is turned on (see
/// [`ToolCatalogues::ensure_current`]). Every change the catalogue may
/// have to follow takes the next number, before its read, and a catalogue
/// is put in place only if it was read for a later change than the one in
/// place: of two rebuilds finishing out of order, the later read stands.
#[derive(Clone)]
pub struct ToolCatalogues {
    inner: Arc<Inner>,
}

struct Inner {
    db: Db,
    store: Arc<GameDataStore>,
    reading_on: ReadingOn,
    cell: Arc<ToolCatalogueCell>,
    /// The latest change noted. The data as it stood at the start is the
    /// first.
    latest: AtomicU64,
    /// The change the catalogue in the cell was read for; 0 before any.
    placed: Mutex<u64>,
}

impl ToolCatalogues {
    /// Nothing is built yet: the cell holds an empty catalogue until the
    /// reading is on and [`ToolCatalogues::ensure_current`] is called.
    pub fn new(db: Db, store: Arc<GameDataStore>, reading_on: ReadingOn) -> Self {
        Self {
            inner: Arc::new(Inner {
                db,
                store,
                reading_on,
                cell: Arc::new(ToolCatalogueCell::default()),
                latest: AtomicU64::new(1),
                placed: Mutex::new(0),
            }),
        }
    }

    /// The cell the watcher reads.
    pub fn cell(&self) -> Arc<ToolCatalogueCell> {
        self.inner.cell.clone()
    }

    /// Something the catalogue is built from changed (an entry added,
    /// edited, deleted, turned on or off; a name dismissed). While the
    /// reading is on the catalogue is rebuilt now; while it is off the
    /// change is only noted.
    pub async fn changed(&self) {
        let change = self.next_change();
        if (self.inner.reading_on)() {
            self.rebuild_for(change).await;
        }
    }

    /// Build the catalogue if the one in place, if any, was read before the
    /// latest change: called when the reading is turned on, before the
    /// watcher can look.
    pub async fn ensure_current(&self) {
        let latest = self.inner.latest.load(Ordering::SeqCst);
        if *self.lock_placed() >= latest {
            return;
        }
        self.rebuild_for(latest).await;
    }

    fn next_change(&self) -> u64 {
        self.inner.latest.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn lock_placed(&self) -> std::sync::MutexGuard<'_, u64> {
        self.inner
            .placed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Read and build the catalogue for `change`, and put it in place
    /// unless one read for a later change is there already. A build that
    /// fails leaves the catalogue as it was.
    async fn rebuild_for(&self, change: u64) {
        match build_catalogue(&self.inner.db, &self.inner.store).await {
            Ok(catalogue) => {
                self.place(change, catalogue);
            }
            Err(error) => tracing::warn!(
                target: "eo::tool_catalogue",
                %error,
                "the held-tool catalogue was not rebuilt"
            ),
        }
    }

    /// Put `catalogue`, read for `change`, in place if no catalogue read for
    /// a later change is there. Answers whether it was put in place.
    fn place(&self, change: u64, catalogue: ToolCatalogue) -> bool {
        let mut placed = self.lock_placed();
        if change <= *placed {
            return false;
        }
        self.inner.cell.replace(catalogue);
        *placed = change;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEAPONS: &str = r#"[
        {"id": 7, "name": "Sollomate Opalo"},
        {"id": "8", "name": "Herman ARK-0"}
    ]"#;

    async fn fixture() -> (tempfile::TempDir, Db, Arc<GameDataStore>) {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = dir.path().join("snapshot");
        std::fs::create_dir_all(&snapshot).unwrap();
        std::fs::write(snapshot.join("weapons.json"), WEAPONS).unwrap();
        let db = Db::open(&dir.path().join("entropia_orme.db"))
            .await
            .unwrap();
        let store = Arc::new(GameDataStore::new(&snapshot).unwrap());
        (dir, db, store)
    }

    async fn add(db: &Db, name: &str, item_type: &str) -> i64 {
        db.insert_equipment(name.into(), item_type.into(), None, "{}".into())
            .await
            .unwrap()
    }

    fn library(catalogue: &ToolCatalogue) -> Vec<(String, ToolKind, bool)> {
        catalogue
            .library()
            .map(|tool| (tool.name.clone(), tool.kind, tool.enabled))
            .collect()
    }

    #[test]
    fn only_what_a_player_can_hold_is_on_the_bar() {
        let row = |kind: &str| (1, "X".to_string(), kind.to_string(), "{}".to_string(), true);
        assert_eq!(holdable(&row("weapon")).unwrap().kind, ToolKind::Weapon);
        assert_eq!(holdable(&row("healing")).unwrap().kind, ToolKind::Healing);
        assert_eq!(holdable(&row("tool")).unwrap().kind, ToolKind::Tool);
        assert_eq!(holdable(&row("consumable")), None);
        let off = (
            2,
            "Y".to_string(),
            "weapon".to_string(),
            "{}".to_string(),
            false,
        );
        assert!(!holdable(&off).unwrap().enabled, "the flag is carried");
    }

    #[tokio::test]
    async fn the_catalogue_holds_the_library_the_dismissed_names_and_the_bundled_tools() {
        let (_dir, db, store) = fixture().await;
        add(&db, "Sollomate Opalo", "weapon").await;
        let healer = add(&db, "Vivo T1", "healing").await;
        add(&db, "Pain Killer", "consumable").await;
        db.set_equipment_enabled(healer, false).await.unwrap();
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO dismissed_tool_names (name, dismissed_at) VALUES ('Teleport Chip', 1)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let catalogue = build_catalogue(&db, &store).await.unwrap();
        assert_eq!(
            library(&catalogue),
            [
                ("Sollomate Opalo".to_string(), ToolKind::Weapon, true),
                ("Vivo T1".to_string(), ToolKind::Healing, false),
            ]
        );
        let bundled: Vec<&str> = catalogue.bundled().map(|t| t.name.as_str()).collect();
        assert_eq!(bundled, ["Sollomate Opalo", "Herman ARK-0"]);
        let names = db.with_reader(|conn| dismissed_names(conn)).await.unwrap();
        assert_eq!(names, ["Teleport Chip"]);
    }

    fn always(on: bool) -> ReadingOn {
        Arc::new(move || on)
    }

    #[tokio::test]
    async fn nothing_is_built_while_the_reading_is_off() {
        let (_dir, db, store) = fixture().await;
        let catalogues = ToolCatalogues::new(db.clone(), store, always(false));
        let cell = catalogues.cell();
        add(&db, "Herman ARK-0", "weapon").await;
        catalogues.changed().await;
        assert!(
            library(&cell.current()).is_empty(),
            "an edit while off builds nothing"
        );
        assert_eq!(
            cell.current().bundled().count(),
            0,
            "not even the bundled tools"
        );
    }

    #[tokio::test]
    async fn turning_the_reading_on_builds_once_from_the_data_as_it_then_stands() {
        let (_dir, db, store) = fixture().await;
        let on = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reading_on: ReadingOn = {
            let on = on.clone();
            Arc::new(move || on.load(Ordering::SeqCst))
        };
        let catalogues = ToolCatalogues::new(db.clone(), store, reading_on);
        let cell = catalogues.cell();
        add(&db, "Herman ARK-0", "weapon").await;
        catalogues.changed().await;

        on.store(true, Ordering::SeqCst);
        catalogues.ensure_current().await;
        assert_eq!(
            library(&cell.current()),
            [("Herman ARK-0".to_string(), ToolKind::Weapon, true)]
        );

        // Current already: a second call reads nothing, so a row written
        // behind its back is not picked up.
        add(&db, "Vivo T1", "healing").await;
        catalogues.ensure_current().await;
        assert_eq!(library(&cell.current()).len(), 1, "built once");

        // On: an edit rebuilds.
        catalogues.changed().await;
        assert_eq!(
            library(&cell.current()).len(),
            2,
            "an edit while on rebuilds"
        );
    }

    #[tokio::test]
    async fn a_rebuild_puts_the_library_as_it_now_stands_in_place() {
        let (_dir, db, store) = fixture().await;
        let catalogues = ToolCatalogues::new(db.clone(), store, always(true));
        let cell = catalogues.cell();
        catalogues.ensure_current().await;
        assert!(library(&cell.current()).is_empty());

        let id = add(&db, "Herman ARK-0", "weapon").await;
        assert!(
            library(&cell.current()).is_empty(),
            "nothing changes until a rebuild"
        );
        catalogues.changed().await;
        assert_eq!(
            library(&cell.current()),
            [("Herman ARK-0".to_string(), ToolKind::Weapon, true)]
        );

        db.set_equipment_enabled(id, false).await.unwrap();
        catalogues.changed().await;
        assert_eq!(
            library(&cell.current()),
            [("Herman ARK-0".to_string(), ToolKind::Weapon, false)]
        );
    }

    #[tokio::test]
    async fn of_two_rebuilds_finishing_out_of_order_the_later_read_stands() {
        let (_dir, db, store) = fixture().await;
        let catalogues = ToolCatalogues::new(db.clone(), store.clone(), always(true));
        let cell = catalogues.cell();
        let id = add(&db, "Herman ARK-0", "weapon").await;

        // A reads the library with the weapon on.
        let a = catalogues.next_change();
        let read_a = build_catalogue(&db, &store).await.unwrap();
        // B reads it after the weapon was turned off.
        db.set_equipment_enabled(id, false).await.unwrap();
        let b = catalogues.next_change();
        let read_b = build_catalogue(&db, &store).await.unwrap();

        // B finishes first, then A.
        assert!(catalogues.place(b, read_b));
        assert!(!catalogues.place(a, read_a), "the older read is dropped");
        assert_eq!(
            library(&cell.current()),
            [("Herman ARK-0".to_string(), ToolKind::Weapon, false)],
            "the disabled weapon stays disabled"
        );
    }
}
