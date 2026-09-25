//! The embedded migration chain and its runner.
//!
//! The ledger table (`_sqlx_migrations`), its column shapes, and its
//! checksum discipline (SHA-384 over the raw migration file bytes) are
//! inherited verbatim from the previous runner, so every database in
//! the wild validates unchanged: the same versions, the same
//! descriptions, the same checksums, byte-for-byte. The runner
//! re-implements the same semantics: validate every applied row
//! against the embedded chain, refuse a checksum mismatch or a
//! previously-failed application, then apply the missing tail each in
//! its own transaction with its ledger row.
//!
//! The chain is embedded at compile time; a unit test asserts the
//! embedded set matches the `migrations/` directory on disk, so a new
//! migration file cannot land without joining the chain here.

use rusqlite::Connection;
use sha2::{Digest, Sha384};

use super::DbError;

/// One embedded migration: the version and description the ledger
/// records (derived from the `NNNN_description.sql` filename exactly as
/// the previous runner derived them) and the raw SQL.
pub(super) struct Migration {
    pub(super) version: i64,
    pub(super) description: &'static str,
    pub(super) sql: &'static str,
}

impl Migration {
    /// The ledger checksum: SHA-384 over the raw migration file bytes.
    pub(super) fn checksum(&self) -> Vec<u8> {
        Sha384::digest(self.sql.as_bytes()).to_vec()
    }
}

/// The migration chain, in application order. Filenames map to ledger
/// rows as `NNNN_snake_description.sql` -> (NNNN, "snake description").
pub(super) static MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "schema baseline",
        sql: include_str!("../../migrations/0001_schema_baseline.sql"),
    },
    Migration {
        version: 2,
        description: "analytical indexes",
        sql: include_str!("../../migrations/0002_analytical_indexes.sql"),
    },
    Migration {
        version: 3,
        description: "session summary read columns",
        sql: include_str!("../../migrations/0003_session_summary_read_columns.sql"),
    },
    Migration {
        version: 4,
        description: "daily rollups",
        sql: include_str!("../../migrations/0004_daily_rollups.sql"),
    },
    Migration {
        version: 5,
        description: "market observations",
        sql: include_str!("../../migrations/0005_market_observations.sql"),
    },
    Migration {
        version: 6,
        description: "harvest events",
        sql: include_str!("../../migrations/0006_harvest_events.sql"),
    },
    Migration {
        version: 7,
        description: "map pins",
        sql: include_str!("../../migrations/0007_map_pins.sql"),
    },
    Migration {
        version: 8,
        description: "map views",
        sql: include_str!("../../migrations/0008_map_views.sql"),
    },
    Migration {
        version: 9,
        description: "map navigation",
        sql: include_str!("../../migrations/0009_map_navigation.sql"),
    },
    Migration {
        version: 10,
        description: "navigation runtime fields",
        sql: include_str!("../../migrations/0010_navigation_runtime_fields.sql"),
    },
    Migration {
        version: 11,
        description: "pin configs",
        sql: include_str!("../../migrations/0011_pin_configs.sql"),
    },
    Migration {
        version: 12,
        description: "harvest stock removed",
        sql: include_str!("../../migrations/0012_harvest_stock_removed.sql"),
    },
    Migration {
        version: 13,
        description: "harvest yield tier",
        sql: include_str!("../../migrations/0013_harvest_yield_tier.sql"),
    },
    Migration {
        version: 14,
        description: "auction sales",
        sql: include_str!("../../migrations/0014_auction_sales.sql"),
    },
    Migration {
        version: 15,
        description: "stock movement tool",
        sql: include_str!("../../migrations/0015_stock_movement_tool.sql"),
    },
    Migration {
        version: 16,
        description: "stock opening balance",
        sql: include_str!("../../migrations/0016_stock_opening_balance.sql"),
    },
    Migration {
        version: 17,
        description: "undone entries",
        sql: include_str!("../../migrations/0017_undone_entries.sql"),
    },
    Migration {
        version: 18,
        description: "session facets",
        sql: include_str!("../../migrations/0018_session_facets.sql"),
    },
    Migration {
        version: 19,
        description: "session intervals",
        sql: include_str!("../../migrations/0019_session_intervals.sql"),
    },
    Migration {
        version: 20,
        description: "signal quests",
        sql: include_str!("../../migrations/0020_signal_quests.sql"),
    },
    Migration {
        version: 21,
        description: "quest families",
        sql: include_str!("../../migrations/0021_quest_families.sql"),
    },
    Migration {
        version: 22,
        description: "session definitions",
        sql: include_str!("../../migrations/0022_session_definitions.sql"),
    },
    Migration {
        version: 23,
        description: "default session definition",
        sql: include_str!("../../migrations/0023_default_session_definition.sql"),
    },
    Migration {
        version: 24,
        description: "hunting stock provenance",
        sql: include_str!("../../migrations/0024_hunting_stock_provenance.sql"),
    },
    Migration {
        version: 25,
        description: "loot item name indexes",
        sql: include_str!("../../migrations/0025_loot_item_name_indexes.sql"),
    },
    Migration {
        version: 26,
        description: "session activity rollups",
        sql: include_str!("../../migrations/0026_session_activity_rollups.sql"),
    },
    Migration {
        version: 27,
        description: "hunting definition provenance",
        sql: include_str!("../../migrations/0027_hunting_definition_provenance.sql"),
    },
    Migration {
        version: 28,
        description: "quest reward provenance",
        sql: include_str!("../../migrations/0028_quest_reward_provenance.sql"),
    },
    Migration {
        version: 29,
        description: "session context loot rollups",
        sql: include_str!("../../migrations/0029_session_context_loot_rollups.sql"),
    },
    Migration {
        version: 30,
        description: "quest reward items",
        sql: include_str!("../../migrations/0030_quest_reward_items.sql"),
    },
    Migration {
        version: 31,
        description: "stock outcomes",
        sql: include_str!("../../migrations/0031_stock_outcomes.sql"),
    },
    Migration {
        version: 32,
        description: "inventory hub",
        sql: include_str!("../../migrations/0032_inventory_hub.sql"),
    },
    Migration {
        version: 33,
        description: "listing duration",
        sql: include_str!("../../migrations/0033_listing_duration.sql"),
    },
    Migration {
        version: 34,
        description: "listing instant",
        sql: include_str!("../../migrations/0034_listing_instant.sql"),
    },
    Migration {
        version: 35,
        description: "quest reward kinds",
        sql: include_str!("../../migrations/0035_quest_reward_kinds.sql"),
    },
    Migration {
        version: 36,
        description: "mixed quest reward kinds",
        sql: include_str!("../../migrations/0036_mixed_quest_reward_kinds.sql"),
    },
    Migration {
        version: 37,
        description: "typed quest rewards",
        sql: include_str!("../../migrations/0037_typed_quest_rewards.sql"),
    },
    Migration {
        version: 38,
        description: "quest runs",
        sql: include_str!("../../migrations/0038_quest_runs.sql"),
    },
    Migration {
        version: 39,
        description: "market unit prices",
        sql: include_str!("../../migrations/0039_market_unit_prices.sql"),
    },
    Migration {
        version: 40,
        description: "quest reward reviews",
        sql: include_str!("../../migrations/0040_quest_reward_reviews.sql"),
    },
    Migration {
        version: 41,
        description: "ARIS unresolved rewards",
        sql: include_str!("../../migrations/0041_ARIS_unresolved_rewards.sql"),
    },
    Migration {
        version: 42,
        description: "quest run ownership",
        sql: include_str!("../../migrations/0042_quest_run_ownership.sql"),
    },
    Migration {
        version: 43,
        description: "protection accounting",
        sql: include_str!("../../migrations/0043_protection_accounting.sql"),
    },
    Migration {
        version: 44,
        description: "deferred protection costs",
        sql: include_str!("../../migrations/0044_deferred_protection_costs.sql"),
    },
    Migration {
        version: 45,
        description: "manual quest hand in",
        sql: include_str!("../../migrations/0045_manual_quest_hand_in.sql"),
    },
    Migration {
        version: 46,
        description: "quest reward lifecycle",
        sql: include_str!("../../migrations/0046_quest_reward_lifecycle.sql"),
    },
    Migration {
        version: 47,
        description: "healing attribution",
        sql: include_str!("../../migrations/0047_healing_attribution.sql"),
    },
    Migration {
        version: 48,
        description: "expected hunting evidence",
        sql: include_str!("../../migrations/0048_expected_hunting_evidence.sql"),
    },
    Migration {
        version: 49,
        description: "expected hunting phase identity",
        sql: include_str!("../../migrations/0049_expected_hunting_phase_identity.sql"),
    },
    Migration {
        version: 50,
        description: "session offensive evidence rollups",
        sql: include_str!("../../migrations/0050_session_offensive_evidence_rollups.sql"),
    },
    Migration {
        version: 51,
        description: "context offensive evidence",
        sql: include_str!("../../migrations/0051_context_offensive_evidence.sql"),
    },
    Migration {
        version: 52,
        description: "protection hit allocation",
        sql: include_str!("../../migrations/0052_protection_hit_allocation.sql"),
    },
    Migration {
        version: 53,
        description: "session protection policy",
        sql: include_str!("../../migrations/0053_session_protection_policy.sql"),
    },
    Migration {
        version: 54,
        description: "session armour cost policy",
        sql: include_str!("../../migrations/0054_session_armour_cost_policy.sql"),
    },
    Migration {
        version: 55,
        description: "session grain protection costs",
        sql: include_str!("../../migrations/0055_session_grain_protection_costs.sql"),
    },
    Migration {
        version: 56,
        description: "protection recording undo",
        sql: include_str!("../../migrations/0056_protection_recording_undo.sql"),
    },
    Migration {
        version: 57,
        description: "healing corrections",
        sql: include_str!("../../migrations/0057_healing_corrections.sql"),
    },
];

// Applied migrations are immutable. These hashes are a deliberate second
// ratchet beside the runtime ledger validation: changing an old SQL file now
// fails locally even when the test database is fresh and has no historic row
// capable of exposing the drift. Schema refinements always add a new version.
#[cfg(test)]
const FROZEN_CHECKSUMS: &[&str] = &[
    "92076B31C71D64008E027CB9016A4495BD477CB53BFAB3738926964A241CED0F8D8B2BB81BE70C673A3F86BFF2ED83CA",
    "4F167CE13DFEEB846931505C633FCF6F5D45E8FA760F3014DD6273796CA7E3DD81ED1A08527C7C7DAC85301B9B46FA10",
    "09ECF202A7D3CF35185195D0C623FB3BCC53CD598B915D8A62CB60EAFF698C266355EC7BB49EF4B2C14513681AD2D732",
    "4008598E5E86F950CA7758016F79D51A9C530D61388D0FBC2B6D080C46219371D1DB1DAA99F5BB3CDC1A7F0D9C9CDA60",
    "8B4DAB6032687FFAE181CEE89731D792A6AF12D122F471FC6AADC391A96C14251B16DCF0C1E3E5FA4701393C3BACB1B9",
    "84AD81FF155635AA349517B71A1264FF75D7D758AD6BD3FFE742DFBA4DFD246B1984613593346EDA6704C846044594DD",
    "E1E390E195B57A2AEE8B4F9D58FDC398FCA2B3200B9473D256D9295D15EE4608E3A9873F4EBC9DAA64BB692802C28B23",
    "667746910B06E0DBC590639E4159B3357316BAFFA22F4B10BEADB4FDF2E015D2E5978A1BDF78B91CEB136B9C489DDA5D",
    "0B3F86B763DB00318691AB7640AAB1901A5FE8A7A6FDE0B1D61B9C71048529DE6119D4E4CFDFD97711B15DFF2287307D",
    "14807806F2AEEA83A890C0114AABEAA2B1DAF3749D335CCFB0EADD3945CEB115C7788FA0FD1D340741FDC951986D677A",
    "D4CA3183B196882C7684EE6819E1761F35F33807EA01D8A63EB16E0E26FDFCEE3D74860C42CFF525D6F9E923B9AF0F0E",
    "E55EA0D16225309C60A08E5EC3B56D25AEA330B05A66C9D7EC97842F3985DCDB93AE74B66A27A5608F95EB891186FA56",
    "810DB5755C5307D254A7A39FEDE6629A930F6855593D9D081FDBCF346814E8AF0EDC020671F62D96B49BCB92F8E4BEED",
    "6F42FEFA98CF2C742D74B0A10F9C8C334C0FFCCCBAD422E13E3F25BEE5DEB88AA48D9767D16D170CCA5209E1DA731E0A",
    "EAC4ADF328EE46DB3F3A16D4DFB5629A004027A24C7F63AD86CE97D3F28535FAE8AFB2E45E166DC8B79708A118526796",
    "DF66ED36758ED3D19D74D142C3CA02712B3435685037C57D15B6790FC226FA71EEE2ACDADEF13C42F59977C14FF23C29",
    "6F4D4FCDB6F696BD1FBAEBFAEFF1A794D09D5FE26F0BB55BA51817B057B29DCAC8E83534881B42FE5FB6671288B1A4E9",
    "451A61961B15890F10954495E2E2E151BBC4AF47B36B498794B64FF65A07C89F420E205AAF7B8817295E74AE7F48C410",
    "147A07C1055FDD09810E13259D24FEBB4E078C2EA9D8C0FF6631943038D8900AC8031AFCD56F17A22C56F00FEEE46799",
    "321911B525CF5B0EFFB1F763563CC3F982F20930DA7B8B4754E5AD777394EDE6F15A7D1419E41BF8058CB70634BED329",
    "E1754050167F73A9B97809EC1399B88C016A5F71422B8684225FD7B834546545A2766FCF0C072BE7089B5DEEA855A41C",
    "EF1309FFA7D79EDEDD0AA4AB9106066C7E6B2CB9856DDE380DB348A5FE9D105C8ADA28D4405594EE3F2988B7E114D3A5",
    "708CB017C278DF637B27B2B8E787A6DC26722B951872932E99C4F50E7974995C14C3065403FD940BF31559D5ED21F3AA",
    "7E02FF79CF14E66EA4F28EAF0D1EEE302BBBE2A14B0E869E237B0FC5A10E8C6AF2BC7FE59880DCF9324E66F70C210226",
    "3EE610D8454477F0967F64E2E23A20451F63A1F598C63E2FD4DCA92B6B449CD2FED2BFC578D3A018F09539344E918A11",
    "FC579385A79892AAA7118C3E565FF81DBC318A77F6C375CEB8EAD0103643B6648A66C0D097FEAC52A8CF6738153E51B4",
    "C60A838C87F764BE99A8AAE4FEA553C202CE00D4C6C80D840B92B06E5111FBAFD76C892F34AC66157EF4A01A7AC015C4",
    "B53CCF5F17EB054A5BD4A9A2CF56D175605D0C804DC26DB3FAC1ED8DA5E42CFF27128ABECA438893BF016F77E4E156B0",
    "2765217EAF4562C4CE0A8E769A35C17528FCA1EF12B7F00E83DFA9333FDC0A08539788D84E06CE626AD0CC69AB334FBB",
    "4FE8D7F709261531F642402F58D06CE651C5BA0E14DA9AA41880F73A44C6C4C92901DF3EDED0EC45DC18CA730FCA7C55",
    "87F131388C669ED061C33E18E671DA1AC57C836FA3E576940F1A837A4D6640057AFC36BDCA340B5B883851F0BFA1F6AD",
    "E9CFFFB264DAC08BD6D69BDAEA80C9588597BA2EB03D8B43ABDBC02FA05813F73E82A7BD13C3B4EC77EBC6DF40C2383B",
    "F914EEACCDF305518BB4A603298140DB63FCABB01BE29A9F3F6D4B84E946F263396B0FE03A2667D91EB7A522D28F96C5",
    "3265E540D74CC0B4CFF006BA9A55FBED9B82EB501AFCD57E75F880D0AE4DB521CAB8DC7E3B94FF671D265EA19745F524",
    "6786181F66812C5ED1EBB8F35ABD1782348FBF69C5B3E1514F599AA51EDC68DF48D83287CDD436AA1A40DB8175C0B0D8",
    "F5DB9E2125A73C6095779380F0737FDB5536F8F42FEEFEDC2CA9ADE88094285C5B634FF654C1D533A4ED57D6FFDF612E",
    "B6A589321ED686339F23D545962583F14E6F43C467448F2A53056A71EF1F8A12799E2D6C710C51AB428D515CEC09A6F3",
    "6ADB01B0EB3D95B8ACF12CCA7BAF468EF5EA15D2567430E370969B206852E783F5B41B898BDB8D898DBB19A347C9AC00",
    "61E62851FE691D20819E9B3E1CF26DE297F804235115F9E414860637415358425AC33A7DA13967E7F411E4E90BF55D2F",
    "92A77163A9BFF7ADB0489E801FD416991DE497FEDCB870C879657FE65A71FA9DA7BDE2D7F1F842629C4C62E84A691E45",
    "6121A2A315E98BB6FC3A479E1CF3E6FCB0CDFE885F5F19DB65166E009111B8E4EDDA1AF555A7B9EE46BDCD521ED8E7FB",
    "9AFF2B725B0CB1EB7BAC63EAEDFE7C33A9FD7F20A4011AB0C45699860FC15B34EF04135174941CEF3CDAD033D3EC6009",
    "F35378D36B254AFCD74A1CC378FB75EE36EF491CFF3475EFB308756228E25703751A7DD63E8ACBDE2DBBDDE34C21FDD0",
    "851CB33C6BD3D4F0458B379FB87238C0CE0CF8E2EA3E82E00CDD2594423621CAB00DE0DF63DB16FC8B52AF401BC5CCD0",
    "E8E3894C6919916669DE0DF626D0453C26BDD3119FF5B2D3D3734D61B3735DD756670210259CC529E37B25840F0B3CAD",
    "831DF480850CCB94220B7AF4A55D47507935331BFCBCD73DA0B27ED30181E96064E5D13D08380A5FD349B151B6844D97",
    "499090EBAE8875A10C6863090B07196C95DDDAB807FDC36CE66B80F8E399C4C9A9F5543150B40D5877A234CCD75F83C5",
    "628FA04BB2B2AF03ED46B934704BFBF4475229C711F1C44E2FE7CFF1B98A61E90B70FE36C3CE5A76616253B814E76160",
    "8111C0C5CD587A8D59CFC38068C2E0F0553DFFB8D63C72F51956591CDFF8AA05F0DCD18F12DF9C920B997F4A3A2FA383",
    "57D24D21E6A64E85F9616AA6D359120C1F191B3817886C6AE90D5E2062BD933405F178119AD29DAF474E5CA059DEBBFF",
    "49B3ABDBA79E1310C89EB54C3182BEB332883A97C6229453C11F5867768146A9D65D8EAFBC4501775D4F01D1AF133A71",
    "3D056CDB492EEC513AE2AB50F1FC3F5A5D4E1060A8E4080E8525ABB2E349EFDAA3D976F3FC3B189342D7B5369B0EBDFC",
    "44F7DCA5C01DE16671062A14175F318FB4EB7720327303E5F9438DD957130F60CF42D3C47910BCA27816B0F05422202A",
    "DE89CC822BFD6A6BB728B669B2F198DB7DD80048D60ACE1957B03F54CF58C5403F5C74C5B6B423899513AB389742C435",
    "828E7C8B43DFA748063DFFF3AE648B9E9DEF29432B6CDA627558FE42B9DC8209B19B14392F495D16C9B014BFF53BCCFF",
    "36A5E18AF53F84859A0F2CECAF44335F37EF5F9159649871D91DE8D65E1CCC6CC50CE5DDB5F1AD3B844AF49EAE0AA56F",
    "F02AAAECFD69A4ED0416CD0B149CC56C5E4149780ED39158BD63A5568F0944899C518692F55896319889DB7D60E56F34",
];

/// The ledger table, exactly as the previous runner created it (and as
/// every database in the wild carries it).
const LEDGER_DDL: &str = "CREATE TABLE IF NOT EXISTS _sqlx_migrations (\
    version BIGINT PRIMARY KEY, description TEXT NOT NULL, \
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
    success BOOLEAN NOT NULL, checksum BLOB NOT NULL, \
    execution_time BIGINT NOT NULL)";

/// Validate the applied ledger against the embedded chain and apply the
/// missing tail. Call on the write connection, after adoption has
/// stamped or refused (see [`super::adopt_or_refuse`]).
pub(super) fn run(connection: &mut Connection) -> Result<(), DbError> {
    connection.execute_batch(LEDGER_DDL)?;

    // Validate the applied ledger as a contiguous prefix of the embedded
    // chain: same versions in the same positions (which refuses both
    // unknown versions and holes that would otherwise apply out of
    // order), descriptions and checksums identical, every application
    // recorded as successful.
    let applied: Vec<(i64, String, Vec<u8>, bool)> = connection
        .prepare(
            "SELECT version, description, checksum, success FROM _sqlx_migrations \
             ORDER BY version",
        )?
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<_, _>>()?;
    for (index, (version, description, checksum, success)) in applied.iter().enumerate() {
        let Some(known) = MIGRATIONS.get(index).filter(|m| m.version == *version) else {
            return Err(DbError::MigrationValidation {
                version: *version,
                problem: "the applied ledger is not a contiguous prefix of the embedded chain",
            });
        };
        if !success {
            return Err(DbError::MigrationValidation {
                version: *version,
                problem: "a previous application of this migration failed",
            });
        }
        if known.description != description {
            return Err(DbError::MigrationValidation {
                version: *version,
                problem: "applied description does not match the embedded migration",
            });
        }
        if known.checksum() != *checksum {
            return Err(DbError::MigrationValidation {
                version: *version,
                problem: "applied checksum does not match the embedded migration",
            });
        }
    }

    // Apply the missing tail (everything past the validated prefix),
    // each migration and its ledger row in one transaction, in order.
    for migration in &MIGRATIONS[applied.len()..] {
        let started = std::time::Instant::now();
        let tx = connection.transaction()?;
        tx.execute_batch(migration.sql)?;
        tx.execute(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES (?1, ?2, TRUE, ?3, ?4)",
            rusqlite::params![
                migration.version,
                migration.description,
                migration.checksum(),
                started.elapsed().as_nanos() as i64,
            ],
        )?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded chain matches the migrations directory on disk:
    /// same files, same version/description derivation, same bytes. A
    /// new migration file fails here until it joins the chain.
    #[test]
    fn embedded_chain_matches_the_migrations_directory() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .expect("migrations directory")
            .map(|entry| entry.expect("directory entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        files.sort();
        assert_eq!(
            files.len(),
            MIGRATIONS.len(),
            "every migration file joins the embedded chain (and nothing extra)"
        );
        for (file, embedded) in files.iter().zip(MIGRATIONS) {
            let stem = file
                .file_stem()
                .and_then(|stem| stem.to_str())
                .expect("utf-8 migration filename");
            let (version, description) = stem
                .split_once('_')
                .expect("NNNN_description migration filename");
            assert_eq!(
                version.parse::<i64>().expect("numeric version prefix"),
                embedded.version,
                "{stem}: version prefix matches the chain"
            );
            assert_eq!(
                description.replace('_', " "),
                embedded.description,
                "{stem}: description matches the chain"
            );
            let bytes = std::fs::read(file).expect("migration bytes");
            assert_eq!(
                bytes,
                embedded.sql.as_bytes(),
                "{stem}: embedded bytes match the file"
            );
        }
    }

    /// A drifted ledger fails validation loudly: a description edit, a
    /// hole in the applied sequence, and an unknown version each refuse
    /// before any migration applies.
    #[test]
    fn drifted_ledgers_are_refused() {
        let assert_refused = |mutation: &str, expected_problem: &str| {
            let mut connection = Connection::open_in_memory().expect("memory database");
            run(&mut connection).expect("fresh chain applies");
            connection.execute_batch(mutation).expect("ledger mutation");
            match run(&mut connection) {
                Err(DbError::MigrationValidation { problem, .. }) => {
                    assert_eq!(problem, expected_problem, "for mutation: {mutation}")
                }
                other => panic!("expected a validation refusal for {mutation}, got {other:?}"),
            }
        };
        assert_refused(
            "UPDATE _sqlx_migrations SET description = 'edited' WHERE version = 2",
            "applied description does not match the embedded migration",
        );
        assert_refused(
            "DELETE FROM _sqlx_migrations WHERE version = 2",
            "the applied ledger is not a contiguous prefix of the embedded chain",
        );
        assert_refused(
            "UPDATE _sqlx_migrations SET version = 99 WHERE version = 4",
            "the applied ledger is not a contiguous prefix of the embedded chain",
        );
        assert_refused(
            "UPDATE _sqlx_migrations SET checksum = X'00' WHERE version = 3",
            "applied checksum does not match the embedded migration",
        );
        assert_refused(
            "UPDATE _sqlx_migrations SET success = FALSE WHERE version = 1",
            "a previous application of this migration failed",
        );
    }

    /// Every migration checksum is frozen, not only checked against a newly
    /// created ledger. This catches an edit before a persistent development
    /// database has to quarantine itself to reveal the drift.
    #[test]
    fn migration_checksums_are_immutable() {
        assert_eq!(MIGRATIONS.len(), FROZEN_CHECKSUMS.len());
        for (migration, expected) in MIGRATIONS.iter().zip(FROZEN_CHECKSUMS) {
            let actual: String = migration
                .checksum()
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect();
            assert_eq!(
                &actual, expected,
                "migration {} is immutable; add a new migration instead",
                migration.version
            );
        }
    }

    #[test]
    fn protection_hit_allocation_upgrade_reweights_history_and_conserves_cost() {
        let mut connection = Connection::open_in_memory().expect("memory database");
        connection.execute_batch(LEDGER_DDL).expect("ledger");
        for migration in &MIGRATIONS[..49] {
            let tx = connection.transaction().expect("migration transaction");
            tx.execute_batch(migration.sql).expect("migration SQL");
            tx.execute(
                "INSERT INTO _sqlx_migrations \
                 (version, description, success, checksum, execution_time) \
                 VALUES (?1, ?2, TRUE, ?3, 0)",
                rusqlite::params![
                    migration.version,
                    migration.description,
                    migration.checksum()
                ],
            )
            .expect("ledger row");
            tx.commit().expect("migration commit");
        }
        connection
            .execute_batch(
                "INSERT INTO tracking_sessions \
                 (id, started_at, ended_at, is_active, armour_cost) VALUES \
                 ('large-damage', 1, 2, 0, 8), ('three-hits', 3, 4, 0, 2); \
                 INSERT INTO protection_cost_windows \
                 (id, kind, cost_ped, status, client_token, created_at) \
                 VALUES (1, 'repair', 10, 'booked', 'historic-window', 5); \
                 INSERT INTO protection_defence_events \
                 (id, session_id, damage, deflected) VALUES \
                 (1, 'large-damage', 80, 0), \
                 (2, 'three-hits', NULL, 1), \
                 (3, 'three-hits', 20, 0), \
                 (4, 'three-hits', 40, 0); \
                 INSERT INTO protection_cost_evidence \
                 (window_id, set_id, defence_event_id) VALUES \
                 (1, NULL, 1), (1, NULL, 2), (1, NULL, 3), (1, NULL, 4); \
                 INSERT INTO protection_cost_allocations \
                 (window_id, session_id, damage_weight, deflection_count, allocation_share, cost_ped) \
                 VALUES (1, 'large-damage', 80, 0, 0.8, 8), \
                        (1, 'three-hits', 60, 1, 0.2, 2); \
                 INSERT INTO protection_cost_context_allocations \
                 (window_id, session_id, context_key, context_id, damage_weight, \
                  deflection_count, allocation_share, cost_ped) VALUES \
                 (1, 'large-damage', -1, NULL, 80, 0, 0.8, 8), \
                 (1, 'three-hits', -1, NULL, 60, 1, 0.2, 2);",
            )
            .expect("historic damage-weighted fixture");

        run(&mut connection).expect("hit-allocation upgrade");

        let rows = connection
            .prepare(
                "SELECT a.session_id, a.hit_count, a.allocation_share, a.cost_ped, s.armour_cost \
                 FROM protection_cost_allocations a \
                 JOIN tracking_sessions s ON s.id = a.session_id \
                 WHERE a.window_id = 1 ORDER BY a.session_id",
            )
            .expect("allocation query")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, f64>(4)?,
                ))
            })
            .expect("allocation rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect allocations");
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].0.as_str(), rows[0].1), ("large-damage", 1));
        assert_eq!((rows[1].0.as_str(), rows[1].1), ("three-hits", 3));
        assert!((rows[0].2 - 0.25).abs() < 1e-12);
        assert!((rows[0].3 - 2.5).abs() < 1e-12);
        assert!((rows[0].4 - 2.5).abs() < 1e-12);
        assert!((rows[1].2 - 0.75).abs() < 1e-12);
        assert!((rows[1].3 - 7.5).abs() < 1e-12);
        assert!((rows[1].4 - 7.5).abs() < 1e-12);
        assert!((rows.iter().map(|row| row.3).sum::<f64>() - 10.0).abs() < 1e-12);
    }

    #[test]
    fn healing_corrections_upgrade_links_each_activation_to_its_confirming_output() {
        let mut connection = Connection::open_in_memory().expect("memory database");
        connection.execute_batch(LEDGER_DDL).expect("ledger");
        for migration in &MIGRATIONS[..56] {
            let tx = connection.transaction().expect("migration transaction");
            tx.execute_batch(migration.sql).expect("migration SQL");
            tx.execute(
                "INSERT INTO _sqlx_migrations \
                 (version, description, success, checksum, execution_time) \
                 VALUES (?1, ?2, TRUE, ?3, 0)",
                rusqlite::params![
                    migration.version,
                    migration.description,
                    migration.checksum()
                ],
            )
            .expect("ledger row");
            tx.commit().expect("migration commit");
        }
        connection
            .execute_batch(
                "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, heal_cost) \
                 VALUES ('s', 1, 100, 0, 0.07); \
                 INSERT INTO healing_activations \
                 (id, session_id, tool_name, intent_at, observed_at, chat_timestamp, cost_ped, \
                  profile_json, provenance) VALUES \
                 ('live', 's', 'Chip', 10, 10, 't', 0.04, '{}', 'direct'), \
                 ('late', 's', 'FAP', 21, 20, 't', 0.03, '{}', 'retrospective'); \
                 INSERT INTO healing_outputs \
                 (id, session_id, activation_id, observed_at, chat_timestamp, amount, \
                  classification, reason) VALUES \
                 ('tick', 's', 'live', 12, 't', 10, 'effect', \
                  'matched an active healing effect window'), \
                 ('confirm', 's', 'live', 10, 't', 30, 'direct', \
                  'confirmed a paid healing activation'), \
                 ('reconciled', 's', 'late', 20, 't', 80, 'direct', \
                  'reconciled with an earlier hotbar occurrence'), \
                 ('loose', 's', NULL, 30, 't', 5, 'unattributed', \
                  'no compatible paid-healer activation');",
            )
            .expect("historic healing evidence");

        run(&mut connection).expect("healing corrections upgrade");

        let links = connection
            .prepare(
                "SELECT id, confirming_output_id, superseded_at, correction_id \
                 FROM healing_activations ORDER BY id",
            )
            .expect("link query")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<f64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .expect("link rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect links");
        assert_eq!(
            links,
            vec![
                (
                    "late".to_string(),
                    Some("reconciled".to_string()),
                    None,
                    None
                ),
                ("live".to_string(), Some("confirm".to_string()), None, None),
            ]
        );
        let corrections: i64 = connection
            .query_row("SELECT COUNT(*) FROM healing_corrections", [], |row| {
                row.get(0)
            })
            .expect("correction count");
        assert_eq!(corrections, 0);
    }

    #[test]
    fn expected_hunting_phase_identity_preserves_equal_cost_loadouts() {
        let mut connection = Connection::open_in_memory().expect("memory database");
        run(&mut connection).expect("fresh chain applies");
        connection
            .execute_batch(
                "INSERT INTO tracking_sessions(id, started_at, is_active) \
                 VALUES('session', 1.0, 0); \
                 INSERT INTO kills(id, session_id, mob_name, timestamp) \
                 VALUES('kill', 'session', 'Atrox', 2.0); \
                 INSERT INTO kill_tool_stats \
                 (kill_id, tool_name, shots_fired, cost_per_shot, \
                  expected_economics_json, evidence_fingerprint) \
                 VALUES \
                 ('kill', 'Rifle', 2, 0.05, '{\"phase\":1}', 'phase-1'), \
                 ('kill', 'Rifle', 3, 0.05, '{\"phase\":2}', 'phase-2');",
            )
            .expect("distinct phases insert");
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM kill_tool_stats", [], |row| row.get(0))
            .expect("phase count");
        assert_eq!(count, 2);
    }

    #[test]
    fn deferred_protection_upgrade_preserves_prior_bookings_and_starts_a_clean_cursor() {
        let mut connection = Connection::open_in_memory().expect("memory database");
        connection.execute_batch(LEDGER_DDL).expect("ledger");
        for migration in &MIGRATIONS[..43] {
            let tx = connection.transaction().expect("migration transaction");
            tx.execute_batch(migration.sql).expect("migration SQL");
            tx.execute(
                "INSERT INTO _sqlx_migrations \
                 (version, description, success, checksum, execution_time) \
                 VALUES (?1, ?2, TRUE, ?3, 0)",
                rusqlite::params![
                    migration.version,
                    migration.description,
                    migration.checksum()
                ],
            )
            .expect("ledger row");
            tx.commit().expect("migration commit");
        }
        connection
            .execute_batch(
                "INSERT INTO tracking_sessions \
                 (id, started_at, ended_at, is_active, armour_cost) \
                 VALUES ('legacy-session', 1, 2, 0, 2.5); \
                 INSERT INTO protection_sets \
                 (id, kind, name, economy_kind, markup_percent, created_at) \
                 VALUES (1, 'armour', 'Legacy limited', 'limited', 125, 1); \
                 INSERT INTO protection_observations \
                 (id, set_id, client_token, tt_value_ped, source, observed_at) \
                 VALUES (1, 1, 'open', 10, 'manual', 1), \
                        (2, 1, 'close', 8, 'manual', 2); \
                 INSERT INTO protection_reconciliations \
                 (set_id, opening_observation_id, closing_observation_id, consumed_tt_ped, \
                  markup_percent, cost_ped, status, session_id, created_at) \
                 VALUES (1, 1, 2, 2, 125, 2.5, 'booked', 'legacy-session', 2);",
            )
            .expect("legacy protection fixture");

        run(&mut connection).expect("v44 upgrade");

        let (window_count, allocation_count, cursor, armour_cost): (i64, i64, i64, f64) =
            connection
                .query_row(
                    "SELECT \
                        (SELECT COUNT(*) FROM protection_cost_windows), \
                        (SELECT COUNT(*) FROM protection_cost_allocations), \
                        (SELECT defence_event_cursor FROM protection_observations WHERE id = 2), \
                        (SELECT armour_cost FROM tracking_sessions WHERE id = 'legacy-session')",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .expect("upgraded protection history");
        assert_eq!((window_count, allocation_count, cursor), (1, 1, 0));
        assert_eq!(
            armour_cost, 2.5,
            "the migration must not book the cost twice"
        );
    }

    /// Every recording remembers how far along the defence-event stream it
    /// reached, so the next recording of its stream offers only the
    /// sessions after it. History gets the position it would have had.
    #[test]
    fn session_grain_upgrade_gives_every_recording_its_stream_position() {
        let mut connection = Connection::open_in_memory().expect("memory database");
        connection.execute_batch(LEDGER_DDL).expect("ledger");
        for migration in &MIGRATIONS[..54] {
            let tx = connection.transaction().expect("migration transaction");
            tx.execute_batch(migration.sql).expect("migration SQL");
            tx.execute(
                "INSERT INTO _sqlx_migrations \
                 (version, description, success, checksum, execution_time) \
                 VALUES (?1, ?2, TRUE, ?3, 0)",
                rusqlite::params![
                    migration.version,
                    migration.description,
                    migration.checksum()
                ],
            )
            .expect("ledger row");
            tx.commit().expect("migration commit");
        }
        connection
            .execute_batch(
                "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) VALUES \
                    ('ended-before', 1, 2, 0), ('still-running', 3, NULL, 1); \
                 INSERT INTO protection_defence_events (id, session_id, damage, deflected) VALUES \
                    (1, 'ended-before', 10, 0), (2, 'ended-before', NULL, 1), \
                    (3, 'still-running', 10, 0), (4, 'still-running', 10, 0); \
                 INSERT INTO protection_sets \
                    (id, kind, name, economy_kind, markup_percent, created_at) \
                    VALUES (1, 'armour', 'Limited', 'limited', 125, 1); \
                 INSERT INTO protection_observations \
                    (id, set_id, client_token, tt_value_ped, source, observed_at, \
                     defence_event_cursor) \
                    VALUES (1, 1, 'open', 10, 'manual', 1, 0), \
                           (2, 1, 'close', 8, 'manual', 5, 3); \
                 INSERT INTO protection_cost_windows \
                    (id, kind, set_id, opening_observation_id, closing_observation_id, \
                     consumed_tt_ped, markup_percent, cost_ped, status, created_at) \
                    VALUES (1, 'limited_decay', 1, 1, 2, 2, 125, 2.5, 'booked', 5); \
                 INSERT INTO protection_cost_windows (id, kind, cost_ped, status, created_at) \
                    VALUES (2, 'repair', 1.0, 'booked', 4), (3, 'repair', 0.5, 'pending', 0.5); \
                 INSERT INTO protection_cost_evidence (window_id, set_id, defence_event_id) \
                    VALUES (2, NULL, 3); \
                 INSERT INTO protection_sets (id, kind, name, economy_kind, created_at) \
                    VALUES (2, 'armour', 'Hyperion', 'unlimited', 1);",
            )
            .expect("pre-session-grain history");

        run(&mut connection).expect("v55 upgrade");

        let cursor = |id: i64| -> i64 {
            connection
                .query_row(
                    "SELECT evidence_cursor FROM protection_cost_windows WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .expect("cursor")
        };
        assert_eq!(cursor(1), 3, "a limited window reached its closing reading");
        assert_eq!(
            cursor(2),
            3,
            "a repair reached past its own hits and every session ended before it"
        );
        assert_eq!(cursor(3), 0, "a repair before any play reached nothing");

        let archived = |id: i64| -> bool {
            connection
                .query_row(
                    "SELECT archived_at IS NOT NULL FROM protection_sets WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .expect("set")
        };
        assert!(
            archived(2),
            "an unlimited set retires with the pooled stream"
        );
        assert!(!archived(1), "a limited set stays in the catalogue");
        connection
            .execute(
                "INSERT INTO protection_sets \
                 (kind, name, economy_kind, markup_percent, created_at) \
                 VALUES ('armour', 'hyperion', 'limited', 140, 2)",
                [],
            )
            .expect("the retired set's name is free for a limited set");
    }

    #[test]
    fn reward_kind_migration_separates_provenance_from_economic_treatment() {
        let mut connection = Connection::open_in_memory().expect("memory database");
        connection.execute_batch(LEDGER_DDL).expect("ledger");
        for migration in &MIGRATIONS[..34] {
            let tx = connection.transaction().expect("migration transaction");
            tx.execute_batch(migration.sql).expect("migration SQL");
            tx.execute(
                "INSERT INTO _sqlx_migrations \
                 (version, description, success, checksum, execution_time) \
                 VALUES (?1, ?2, TRUE, ?3, 0)",
                rusqlite::params![
                    migration.version,
                    migration.description,
                    migration.checksum()
                ],
            )
            .expect("ledger row");
            tx.commit().expect("migration commit");
        }

        connection
            .execute(
                "INSERT INTO quests(id, name) VALUES (1, 'Reward fixture')",
                [],
            )
            .expect("quest fixture");

        for (session, source, ped) in [
            ("included", "tracked_loot", None),
            ("item", "tracked_loot", None),
            ("liquid", "ledger", Some(2.0)),
            ("mixed", "ledger", Some(2.0)),
        ] {
            connection
                .execute(
                    "INSERT INTO session_quest_completions \
                     (session_id, quest_id, reward_source, reward_ped) \
                     VALUES (?1, 1, ?2, ?3)",
                    rusqlite::params![session, source, ped],
                )
                .expect("completion");
        }
        connection
            .execute(
                "INSERT INTO session_quest_completion_reward_items \
                 (completion_id, item_name, quantity, value_ped) \
                 SELECT id, 'Hyperion Daily Voucher', 1, 0 \
                 FROM session_quest_completions WHERE session_id IN ('item', 'mixed')",
                [],
            )
            .expect("reward items");

        run(&mut connection).expect("reward-kind migration");

        let rows = connection
            .prepare(
                "SELECT session_id, reward_source, reward_kind \
                 FROM session_quest_completions ORDER BY id",
            )
            .expect("query")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .expect("rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect");
        assert_eq!(
            rows,
            [
                (
                    "included".into(),
                    "tracked_loot".into(),
                    "included_in_loot".into()
                ),
                ("item".into(), "tracked_loot".into(), "item".into()),
                ("liquid".into(), "ledger".into(), "fixed_liquid".into()),
                ("mixed".into(), "ledger".into(), "fixed_liquid".into()),
            ]
        );
    }

    #[test]
    fn harvest_yield_migration_backfills_direct_inferred_conflict_and_unknown_rows() {
        let mut connection = Connection::open_in_memory().expect("memory database");
        connection.execute_batch(LEDGER_DDL).expect("ledger");
        for migration in &MIGRATIONS[..12] {
            let tx = connection.transaction().expect("migration transaction");
            tx.execute_batch(migration.sql).expect("migration SQL");
            tx.execute(
                "INSERT INTO _sqlx_migrations \
                 (version, description, success, checksum, execution_time) \
                 VALUES (?1, ?2, TRUE, ?3, 0)",
                rusqlite::params![
                    migration.version,
                    migration.description,
                    migration.checksum()
                ],
            )
            .expect("ledger row");
            tx.commit().expect("migration commit");
        }
        connection
            .execute(
                "INSERT INTO tracking_sessions(id,started_at,ended_at) \
                 VALUES('session',0,300)",
                [],
            )
            .expect("session");
        connection
            .execute(
                "INSERT INTO tracking_sessions(id,started_at,ended_at) \
                 VALUES('other-session',0,300)",
                [],
            )
            .expect("other session");
        for (id, timestamp, tool) in [
            ("short", 0.0, Some("PH-3")),
            ("conflict", 10.0, Some("PH-3")),
            ("huge", 20.0, Some("PH-3")),
            ("long", 50.0, Some("PH-3")),
            ("after", 60.0, Some("PH-3")),
            ("other-tool", 55.0, Some("PH-4")),
            ("isolated", 200.0, Some("PH-3")),
        ] {
            connection
                .execute(
                    "INSERT INTO harvest_events \
                     (id,session_id,timestamp,success,tool_name,cost_ped,loot_total_ped) \
                     VALUES(?1,'session',?2,1,?3,0.1,0)",
                    rusqlite::params![id, timestamp, tool],
                )
                .expect("harvest");
        }
        connection
            .execute(
                "INSERT INTO harvest_events \
                 (id,session_id,timestamp,success,tool_name,cost_ped,loot_total_ped) \
                 VALUES('other-session-row','other-session',55,1,'PH-3',0.1,0)",
                [],
            )
            .expect("other-session harvest");
        for (harvest, item, deactivated_at) in [
            ("short", "Short Moonleaf Board", Some(1.0)),
            ("huge", "Long Moonleaf Board", None),
            ("long", "Moonleaf Board", None),
        ] {
            connection
                .execute(
                    "INSERT INTO harvest_loot_items \
                     (harvest_id,item_name,quantity,value_ped,deactivated_at) \
                     VALUES(?1,?2,1,0.1,?3)",
                    rusqlite::params![harvest, item, deactivated_at],
                )
                .expect("loot");
        }

        run(&mut connection).expect("yield migration");

        let mut stmt = connection
            .prepare(
                "SELECT id,yield_tier,yield_tier_source \
                 FROM harvest_events ORDER BY timestamp,id",
            )
            .expect("query");
        let rows: Vec<(String, String, Option<String>)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("rows")
            .collect::<rusqlite::Result<_>>()
            .expect("collect");
        assert_eq!(
            rows,
            vec![
                ("short".into(), "short".into(), Some("board".into())),
                ("conflict".into(), "unknown".into(), None),
                ("huge".into(), "huge".into(), Some("board".into())),
                ("long".into(), "long".into(), Some("board".into())),
                ("other-session-row".into(), "unknown".into(), None),
                ("other-tool".into(), "unknown".into(), None),
                ("after".into(), "long".into(), Some("inferred".into())),
                ("isolated".into(), "unknown".into(), None),
            ]
        );
    }

    /// The board-to-tier rule exists twice: as Rust in `yield_tier_for_board`
    /// for live attribution, and as SQL inside migration 0013 for the
    /// historical backfill. The migration's bytes are frozen, so only the Rust
    /// side can drift, and a divergence would classify history differently
    /// from live tracking without failing anything.
    ///
    /// This applies the real migration rather than a transcription of its
    /// CASE, so the two implementations are compared as shipped. Each name
    /// gets its own session so no row can inherit a tier from a neighbour and
    /// mask a classification difference.
    #[test]
    fn the_backfill_sql_classifies_boards_exactly_as_the_rust_classifier_does() {
        use crate::harvest_yield::yield_tier_for_board;

        const NAMES: &[&str] = &[
            "Short Moonleaf Board",
            "Moonleaf Board",
            "Long Moonleaf Board",
            "Long Kaisenbrandt Board",
            // A species whose name merely begins with "Long": the space in the
            // "Long " prefix is the whole distinction.
            "Longleaf Board",
            " Board",
            // Non-board loot yields no tier evidence at all.
            "Wood Shavings",
            "Animal Muscle Oil",
        ];

        let mut connection = Connection::open_in_memory().expect("memory database");
        connection.execute_batch(LEDGER_DDL).expect("ledger");
        for migration in &MIGRATIONS[..12] {
            let tx = connection.transaction().expect("migration transaction");
            tx.execute_batch(migration.sql).expect("migration SQL");
            tx.execute(
                "INSERT INTO _sqlx_migrations \
                 (version, description, success, checksum, execution_time) \
                 VALUES (?1, ?2, TRUE, ?3, 0)",
                rusqlite::params![
                    migration.version,
                    migration.description,
                    migration.checksum()
                ],
            )
            .expect("ledger row");
            tx.commit().expect("migration commit");
        }

        for (index, name) in NAMES.iter().enumerate() {
            let session = format!("s{index}");
            let harvest = format!("h{index}");
            connection
                .execute(
                    "INSERT INTO tracking_sessions(id,started_at,ended_at) VALUES(?1,0,300)",
                    rusqlite::params![session],
                )
                .expect("session");
            connection
                .execute(
                    "INSERT INTO harvest_events \
                     (id,session_id,timestamp,success,tool_name,cost_ped,loot_total_ped) \
                     VALUES(?1,?2,10.0,1,'PH-3',0.1,0.1)",
                    rusqlite::params![harvest, session],
                )
                .expect("harvest");
            connection
                .execute(
                    "INSERT INTO harvest_loot_items \
                     (harvest_id,item_name,quantity,value_ped,deactivated_at) \
                     VALUES(?1,?2,1,0.1,NULL)",
                    rusqlite::params![harvest, name],
                )
                .expect("loot");
        }

        run(&mut connection).expect("yield migration");

        for (index, name) in NAMES.iter().enumerate() {
            let (tier, source): (String, Option<String>) = connection
                .query_row(
                    "SELECT yield_tier, yield_tier_source FROM harvest_events WHERE id = ?1",
                    rusqlite::params![format!("h{index}")],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("classified row");

            let rust = yield_tier_for_board(name);
            let expected_tier = rust.map_or("unknown", |t| t.as_str());
            let expected_source = rust.map(|_| "board".to_string());
            assert_eq!(
                (tier.as_str(), source.clone()),
                (expected_tier, expected_source),
                "{name:?} classifies differently in the migration SQL and in Rust"
            );
        }
    }

    /// A database that applied the original navigation migration upgrades
    /// forward without ledger surgery, preserving existing rows and filling
    /// the new route hotkey from its declared default.
    #[test]
    fn navigation_runtime_fields_upgrade_from_the_frozen_v9_schema() {
        let mut connection = Connection::open_in_memory().expect("memory database");
        connection.execute_batch(LEDGER_DDL).expect("ledger");
        for migration in &MIGRATIONS[..9] {
            let tx = connection.transaction().expect("migration transaction");
            tx.execute_batch(migration.sql).expect("migration SQL");
            tx.execute(
                "INSERT INTO _sqlx_migrations \
                 (version, description, success, checksum, execution_time) \
                 VALUES (?1, ?2, TRUE, ?3, 0)",
                rusqlite::params![
                    migration.version,
                    migration.description,
                    migration.checksum()
                ],
            )
            .expect("ledger row");
            tx.commit().expect("migration commit");
        }
        connection
            .execute(
                "INSERT INTO navigation_runs \
                 (planet, status, start_lon, start_lat, current_lon, current_lat, \
                  hop_count, created_at, updated_at) \
                 VALUES ('calypso', 'active', 1, 2, 1, 2, 3, 4, 4)",
                [],
            )
            .expect("v9 navigation row");

        run(&mut connection).expect("v10 upgrade");

        let upgraded: (Option<f64>, String) = connection
            .query_row(
                "SELECT last_position_at, hotkey FROM navigation_runs",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("upgraded navigation row");
        assert_eq!(upgraded, (None, "f8".to_owned()));
        let ledger_tail: i64 = connection
            .query_row("SELECT MAX(version) FROM _sqlx_migrations", [], |row| {
                row.get(0)
            })
            .expect("ledger tail");
        assert_eq!(
            ledger_tail,
            MIGRATIONS.last().expect("chain is non-empty").version
        );
    }
}
