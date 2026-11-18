//! Development-only auction-fee sampling.
//!
//! A live research session turns successive reads of the auction's sell
//! queue into a reproducible dataset: per capture, one PNG preserving the
//! game window's pixels and one versioned JSONL observation, and one
//! analysis-friendly CSV line per queued row read. It never creates a
//! listing or writes accounting state.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::clock::Clock;
use crate::game_ui::sale_queue::{
    Buyout, FeeDisagreement, ItemName, QueueCount, QueuedItem, SaleQueue,
};
use crate::game_ui::validate::without_unit;
use crate::sale_capture::{QueueCapture, SaleCaptureService};
use crate::screen_capture::frame_to_png;

const SCHEMA_VERSION: u32 = 2;
const STUDY_DIR: &str = "auction-fee-study";
const CSV_HEADER: &str = "schema_version,sample,row,captured_at_local,item_name,quantity,tt_value,auction_fee,auction_days,starting_bid,buyout,markup,fee_agrees,accepted,error,screenshot,sha256\n";
const COLLECTION_PROTOCOL: &str = r#"# Auction fee capture protocol

Do not submit a listing. This session only reads the quotes shown by the sell queue.

1. Open the Auction sell window with its sell queue in view and prices stated in PED.
2. Change one input at a time, wait for the quoted fees to settle, then press Space once. Every queued item in view is recorded.
3. Build a price sweep for one fixed stack: sample densely around the 0.50 PED minimum-fee knee, then at progressively wider opening bids.
4. Repeat price sweeps across stacks near 0.10, 1, 5, 10, 50, 100, and 500 PED TT where held items permit.
5. At one fixed TT and opening bid, vary duration across 1, 7, and 14 days. At another fixed point, vary buyout while leaving the opening bid unchanged.
6. Where practical, capture the same TT and bid using different item identities or quantities.
7. Repeat at least ten unchanged scenarios. Exact repeats reveal quote or reading noise.

A queued row whose every figure read and whose every check held is recorded with its figures, `fee_agrees` true. A row whose every figure read but whose fee is not the fee the app's model gives for its opening bid is recorded too, with every figure as the window printed it and `fee_agrees` false: that is the observation this study exists for. Any other row is recorded in `samples.jsonl` as unread, with why, and without figures.

`samples.csv` is the modelling table, one line per queued row recorded with its figures (a row whose fee disagrees is numbered `unread-N`); `samples.jsonl` preserves the complete structured read of each capture; each line names and hashes its source PNG.
"#;

#[derive(Debug, Clone, PartialEq)]
pub struct ResearchCapture {
    pub sample: u64,
    pub accepted: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResearchStatus {
    pub active: bool,
    pub busy: bool,
    pub sample_count: u64,
    pub output_dir: Option<PathBuf>,
    pub last_capture: Option<ResearchCapture>,
}

#[derive(Debug, Error)]
pub enum ResearchError {
    #[error("auction-fee research is available only in development builds")]
    Unavailable,
    #[error("auction-fee research is not active")]
    Inactive,
    #[error("auction-fee research is already capturing")]
    Busy,
    #[error("auction-fee research could not write its dataset: {0}")]
    Io(#[from] std::io::Error),
    #[error("the captured frame could not be encoded")]
    Encode,
}

struct State {
    active: bool,
    busy: bool,
    generation: u64,
    sample_count: u64,
    output_dir: Option<PathBuf>,
    last_capture: Option<ResearchCapture>,
}

struct PreparedCapture {
    capture: ResearchCapture,
    png: Vec<u8>,
    screenshot: String,
    sha256: String,
    captured_at: String,
    read: QueueCapture,
}

/// One actor-owned research session over the production sell-queue
/// capture.
pub struct AuctionFeeResearchService {
    sale_capture: Arc<SaleCaptureService>,
    clock: Arc<dyn Clock>,
    root: PathBuf,
    state: Mutex<State>,
}

impl AuctionFeeResearchService {
    pub fn new(
        sale_capture: Arc<SaleCaptureService>,
        clock: Arc<dyn Clock>,
        data_dir: &Path,
    ) -> Arc<Self> {
        Arc::new(Self {
            sale_capture,
            clock,
            root: data_dir.join("debug").join(STUDY_DIR),
            state: Mutex::new(State {
                active: false,
                busy: false,
                generation: 0,
                sample_count: 0,
                output_dir: None,
                last_capture: None,
            }),
        })
    }

    pub fn start(&self) -> Result<ResearchStatus, ResearchError> {
        if !cfg!(debug_assertions) {
            return Err(ResearchError::Unavailable);
        }
        let mut state = self.state.lock().expect("auction fee research state");
        if state.active {
            return Ok(status(&state));
        }

        std::fs::create_dir_all(&self.root)?;
        let stem = self.clock.now().format("%Y%m%dT%H%M%S").to_string();
        let output_dir = unique_session_dir(&self.root, &stem);
        std::fs::create_dir(&output_dir)?;
        std::fs::write(output_dir.join("samples.csv"), CSV_HEADER)?;
        std::fs::write(output_dir.join("README.md"), COLLECTION_PROTOCOL)?;
        let manifest = json!({
            "schema_version": SCHEMA_VERSION,
            "created_at_local": self.clock.now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string(),
            "purpose": "Auction fee curve research; no listing or accounting mutation is performed",
            "files": {
                "observations": "samples.jsonl",
                "analysis_table": "samples.csv",
                "screenshots": "sample-NNNN.png"
            }
        });
        std::fs::write(
            output_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).expect("research manifest serialises"),
        )?;

        state.active = true;
        state.busy = false;
        state.generation = state.generation.wrapping_add(1);
        state.sample_count = 0;
        state.output_dir = Some(output_dir);
        state.last_capture = None;
        Ok(status(&state))
    }

    pub fn stop(&self) -> ResearchStatus {
        let mut state = self.state.lock().expect("auction fee research state");
        state.active = false;
        state.busy = false;
        state.generation = state.generation.wrapping_add(1);
        status(&state)
    }

    pub fn status(&self) -> ResearchStatus {
        status(&self.state.lock().expect("auction fee research state"))
    }

    pub fn is_active(&self) -> bool {
        self.state
            .lock()
            .expect("auction fee research state")
            .active
    }

    /// Take and persist one sample. The busy flag is acquired before the
    /// blocking screen/OCR work and cleared on every result.
    pub fn capture(&self) -> Result<ResearchCapture, ResearchError> {
        let (sample, output_dir, generation) = {
            let mut state = self.state.lock().expect("auction fee research state");
            if !state.active {
                return Err(ResearchError::Inactive);
            }
            if state.busy {
                return Err(ResearchError::Busy);
            }
            state.busy = true;
            (
                state.sample_count + 1,
                state
                    .output_dir
                    .clone()
                    .expect("active research output dir"),
                state.generation,
            )
        };

        let result = self.prepare_capture(sample);
        let mut state = self.state.lock().expect("auction fee research state");
        if !state.active || state.generation != generation {
            return Err(ResearchError::Inactive);
        }
        match &result {
            Ok(prepared) => {
                // Hold the session lock across the append transaction. Stop
                // either wins before this point, causing the generation check
                // above to discard the pixels, or waits until every file is
                // durable. Once stop returns, no old capture can write later.
                if let Err(error) = self.persist_capture(&output_dir, sample, prepared) {
                    state.busy = false;
                    state.last_capture = Some(ResearchCapture {
                        sample: state.sample_count,
                        accepted: false,
                        message: error.to_string(),
                    });
                    return Err(error);
                }
                state.busy = false;
                state.sample_count = sample;
                state.last_capture = Some(prepared.capture.clone());
            }
            Err(error) => {
                state.busy = false;
                state.last_capture = Some(ResearchCapture {
                    sample: state.sample_count,
                    accepted: false,
                    message: error.to_string(),
                });
            }
        }
        result.map(|prepared| prepared.capture)
    }

    fn prepare_capture(&self, sample: u64) -> Result<PreparedCapture, ResearchError> {
        let (read, frame) = self.sale_capture.observe();
        let frame = match (frame, &read.outcome) {
            (Some(frame), _) => frame,
            (None, Err(refusal)) => {
                return Err(ResearchError::Io(std::io::Error::other(
                    refusal.to_string(),
                )))
            }
            (None, Ok(_)) => return Err(ResearchError::Encode),
        };
        let png = frame_to_png(&frame).ok_or(ResearchError::Encode)?;
        let screenshot = format!("sample-{sample:04}.png");
        let sha256 = format!("{:x}", Sha256::digest(&png));
        // A reading is dated to when its screen was taken.
        let moment = match &read.outcome {
            Ok(reading) => reading.read_at,
            Err(_) => self.clock.now(),
        };
        let captured_at = moment.format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
        let rows = read.outcome.as_ref().map_or(0, |r| r.queue.rows.len());
        let disagreeing = read.outcome.as_ref().map_or(0, |r| {
            r.queue
                .unread
                .iter()
                .filter(|row| row.fee_disagreement.is_some())
                .count()
        });
        let accepted = rows + disagreeing > 0;
        let message = match &read.outcome {
            Ok(_) if disagreeing > 0 => format!(
                "Sample {sample} saved: {rows} queued rows read, {disagreeing} with a fee the model does not give"
            ),
            Ok(_) if accepted => format!("Sample {sample} saved: {rows} queued rows read"),
            Ok(_) => format!("Sample {sample} saved with no queued row read"),
            Err(refusal) => format!("Sample {sample} saved unread: {refusal}"),
        };
        Ok(PreparedCapture {
            capture: ResearchCapture {
                sample,
                accepted,
                message,
            },
            png,
            screenshot,
            sha256,
            captured_at,
            read,
        })
    }

    fn persist_capture(
        &self,
        output_dir: &Path,
        sample: u64,
        prepared: &PreparedCapture,
    ) -> Result<(), ResearchError> {
        std::fs::write(output_dir.join(&prepared.screenshot), &prepared.png)?;
        let record = json!({
            "schema_version": SCHEMA_VERSION,
            "sample": sample,
            "captured_at_local": prepared.captured_at,
            "accepted": prepared.capture.accepted,
            "screenshot": prepared.screenshot,
            "sha256": prepared.sha256,
            "capture": capture_record(&prepared.read),
        });
        append_line(
            &output_dir.join("samples.jsonl"),
            &serde_json::to_string(&record).expect("research record serialises"),
        )?;
        for line in csv_lines(sample, prepared) {
            append_line(&output_dir.join("samples.csv"), &line)?;
        }
        Ok(())
    }
}

fn status(state: &State) -> ResearchStatus {
    ResearchStatus {
        active: state.active,
        busy: state.busy,
        sample_count: state.sample_count,
        output_dir: state.output_dir.clone(),
        last_capture: state.last_capture.clone(),
    }
}

fn unique_session_dir(root: &Path, stem: &str) -> PathBuf {
    let first = root.join(stem);
    if !first.exists() {
        return first;
    }
    for suffix in 2..=9999 {
        let candidate = root.join(format!("{stem}-{suffix:02}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    root.join(format!("{stem}-overflow"))
}

fn append_line(path: &Path, line: &str) -> Result<(), std::io::Error> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")?;
    file.flush()
}

/// The structured read of one capture: every queued row read, with its
/// figures as printed and as numbers; why each unread row did not read;
/// and the queue's count. A capture that read no queue says why.
fn capture_record(capture: &QueueCapture) -> Value {
    match &capture.outcome {
        Err(refusal) => json!({ "refused": refusal.to_string() }),
        Ok(reading) => queue_record(&reading.queue),
    }
}

fn queue_record(queue: &SaleQueue) -> Value {
    let count = match queue.count {
        QueueCount::Complete(n) => json!({ "complete": n }),
        QueueCount::Incomplete {
            heading,
            read,
            unread,
            partial,
        } => json!({
            "incomplete": { "heading": heading, "read": read, "unread": unread, "partial": partial }
        }),
        QueueCount::Contradicted { heading, seen } => {
            json!({ "contradicted": { "heading": heading, "seen": seen } })
        }
        QueueCount::Unread => json!("unread"),
    };
    json!({
        "rows": queue.rows.iter().map(row_record).collect::<Vec<_>>(),
        "unread": queue.unread.iter().map(|row| json!({
            "raw_name": row.raw_name,
            "refusals": row.refusals.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "fee_disagrees": row.fee_disagreement.is_some(),
            "printed": row.fee_disagreement.as_ref().map(|printed| json!({
                "item_name": printed.name,
                "tt_value": printed.tt,
                "quantity": printed.quantity,
                "starting_bid": printed.opening_bid,
                "buyout": printed.buyout,
                "auction_days": printed.days,
                "auction_fee": printed.fee,
            })),
        })).collect::<Vec<_>>(),
        "partial": queue.partial,
        "count": count,
    })
}

fn row_record(row: &QueuedItem) -> Value {
    let evidence = match &row.name {
        ItemName::Catalogued(_) => "catalogued",
        ItemName::Listed(_) => "listed",
        ItemName::Unconfirmed(_) => "unconfirmed",
    };
    json!({
        "item_name": row.name.text(),
        "name_evidence": evidence,
        "tt_value": { "text": row.tt.text, "ped": row.tt.ped },
        "quantity": { "text": row.quantity.text, "value": row.quantity.value },
        "starting_bid": { "text": row.opening_bid.text, "value": row.opening_bid.value },
        "buyout": match &row.buyout {
            Buyout::Off => Value::Null,
            Buyout::Price(price) => json!({ "text": price.text, "value": price.value }),
        },
        "auction_days": { "text": row.days.text, "value": row.days.value },
        "auction_fee": { "text": row.fee.text, "ped": row.fee.ped },
        "markup": row.markup.text,
    })
}

/// The capture's lines of the modelling table: one per queued row read,
/// or one line saying why none was.
fn csv_lines(sample: u64, prepared: &PreparedCapture) -> Vec<String> {
    // A figure as printed, as a number: without its unit or its commas.
    let number = |text: &str| without_unit(text).replace(',', "");
    let line = |index: String, cells: [String; 9], error: &str| {
        [
            SCHEMA_VERSION.to_string(),
            sample.to_string(),
            index,
            csv_cell(&prepared.captured_at),
        ]
        .into_iter()
        .chain(cells)
        .chain([
            prepared.capture.accepted.to_string(),
            csv_cell(error),
            csv_cell(&prepared.screenshot),
            prepared.sha256.clone(),
        ])
        .collect::<Vec<_>>()
        .join(",")
    };
    let read_row = |index: usize, row: &QueuedItem| {
        line(
            (index + 1).to_string(),
            [
                csv_cell(row.name.text()),
                row.quantity.value.to_string(),
                row.tt.ped.to_string(),
                row.fee.ped.to_string(),
                row.days.value.to_string(),
                row.opening_bid.value.to_string(),
                match &row.buyout {
                    Buyout::Off => String::new(),
                    Buyout::Price(price) => price.value.to_string(),
                },
                csv_cell(&row.markup.text),
                "true".into(),
            ],
            "",
        )
    };
    // A row whose only failed check is its fee: every figure as printed,
    // the fee flagged as disagreeing with the model.
    let disagreeing = |index: usize, row: &FeeDisagreement| {
        line(
            format!("unread-{}", index + 1),
            [
                csv_cell(&row.name),
                number(&row.quantity),
                number(&row.tt),
                number(&row.fee),
                number(&row.days),
                number(&row.opening_bid),
                row.buyout.as_deref().map(number).unwrap_or_default(),
                String::new(),
                "false".into(),
            ],
            "the fee is not the model's for the opening bid",
        )
    };
    let none = |error: &str| line(String::new(), Default::default(), error);
    match &prepared.read.outcome {
        Err(refusal) => vec![none(&refusal.to_string())],
        Ok(reading) => {
            let lines: Vec<String> = reading
                .queue
                .rows
                .iter()
                .enumerate()
                .map(|(index, row)| read_row(index, row))
                .chain(
                    reading
                        .queue
                        .unread
                        .iter()
                        .enumerate()
                        .filter_map(|(index, row)| {
                            row.fee_disagreement
                                .as_ref()
                                .map(|row| disagreeing(index, row))
                        }),
                )
                .collect();
            if lines.is_empty() {
                vec![none("no queued row read")]
            } else {
                lines
            }
        }
    }
}

fn csv_cell(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::MockClock;
    use crate::game_ui::test_support::{
        bundled_profile, sale_window, standard_sale_layout, window_frame,
    };
    use crate::sale_capture::SaleCaptureProviders;
    use std::sync::Barrier;

    fn service(root: &Path) -> Arc<AuctionFeeResearchService> {
        service_with_gate(root, None)
    }

    /// A research session over a drawn sell window, its capture held at
    /// `gate` (when given) until the test releases it.
    fn service_with_gate(
        root: &Path,
        gate: Option<(Arc<Barrier>, Arc<Barrier>)>,
    ) -> Arc<AuctionFeeResearchService> {
        let drawn = sale_window(&standard_sale_layout());
        let image = drawn.image.clone();
        let clock = Arc::new(MockClock::new(None, 0.0));
        let sale_capture = Arc::new(SaleCaptureService::new(
            SaleCaptureProviders {
                frame: Arc::new(move |area| {
                    if let Some((reached, release)) = &gate {
                        reached.wait();
                        release.wait();
                    }
                    window_frame(&image, (0, 0), area)
                }),
                profile: Some(Arc::new(bundled_profile())),
                read_text: Some(Arc::new(drawn.recogniser())),
                ..Default::default()
            },
            clock.clone(),
        ));
        AuctionFeeResearchService::new(sale_capture, clock, root)
    }

    #[test]
    fn a_capture_writes_hashed_evidence_rows_without_mutating_any_domain_state() {
        let temp = tempfile::tempdir().unwrap();
        let service = service(temp.path());
        let started = service.start().unwrap();
        assert!(started.active);

        let capture = service.capture().unwrap();
        assert!(capture.accepted);
        let status = service.stop();
        assert_eq!(status.sample_count, 1);
        let dir = status.output_dir.unwrap();
        assert!(dir.join("sample-0001.png").is_file());
        assert!(std::fs::read_to_string(dir.join("README.md"))
            .unwrap()
            .contains("sell queue in view"));
        let jsonl = std::fs::read_to_string(dir.join("samples.jsonl")).unwrap();
        let record: Value = serde_json::from_str(jsonl.trim()).unwrap();
        let rows = &record["capture"]["rows"];
        assert_eq!(rows.as_array().unwrap().len(), 3);
        assert_eq!(rows[0]["tt_value"]["text"], "11.52 PED");
        assert_eq!(rows[0]["auction_fee"]["ped"], 7.33);
        assert_eq!(rows[0]["starting_bid"]["value"], 162);
        assert_eq!(rows[1]["buyout"], Value::Null);
        assert_eq!(record["capture"]["count"], json!({"complete": 3}));
        assert_eq!(record["accepted"], true);
        assert_eq!(record["sha256"].as_str().unwrap().len(), 64);
        let csv = std::fs::read_to_string(dir.join("samples.csv")).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], CSV_HEADER.trim_end());
        assert_eq!(lines.len(), 4, "the header and one line per queued row");
        assert!(lines[1].contains("\"Electric Attack Nanochip 3\",1,11.52,7.33,7,162,172,"));
        assert!(lines[2].contains("\"Output Amplifier Component\",52,20.8,1.54,7,42,,"));
    }

    #[test]
    fn a_fee_the_model_does_not_give_is_recorded_with_its_printed_figures() {
        let temp = tempfile::tempdir().unwrap();
        let drawn = sale_window(&standard_sale_layout());
        let image = drawn.image.clone();
        let inner = drawn.recogniser();
        let clock = Arc::new(MockClock::new(None, 0.0));
        // The first row's fee printed a PEC above what the model gives.
        let sale_capture = Arc::new(SaleCaptureService::new(
            SaleCaptureProviders {
                frame: Arc::new(move |area| window_frame(&image, (0, 0), area)),
                profile: Some(Arc::new(bundled_profile())),
                read_text: Some(Arc::new(move |crop| {
                    inner(crop).map(|(text, c)| {
                        (
                            if text == "7.33 PED" {
                                "7.38 PED".into()
                            } else {
                                text
                            },
                            c,
                        )
                    })
                })),
                ..Default::default()
            },
            clock.clone(),
        ));
        let service = AuctionFeeResearchService::new(sale_capture, clock, temp.path());
        service.start().unwrap();
        let capture = service.capture().unwrap();
        assert!(capture.accepted);
        assert!(capture
            .message
            .contains("1 with a fee the model does not give"));
        let dir = service.stop().output_dir.unwrap();

        let jsonl = std::fs::read_to_string(dir.join("samples.jsonl")).unwrap();
        let record: Value = serde_json::from_str(jsonl.trim()).unwrap();
        let unread = &record["capture"]["unread"][0];
        assert_eq!(unread["fee_disagrees"], true);
        assert_eq!(
            unread["printed"],
            json!({
                "item_name": "Electric Attack Nanochip 3",
                "tt_value": "11.52 PED",
                "quantity": "1",
                "starting_bid": "162",
                "buyout": "172",
                "auction_days": "7",
                "auction_fee": "7.38 PED",
            })
        );
        let csv = std::fs::read_to_string(dir.join("samples.csv")).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 4, "two rows read and one whose fee disagrees");
        assert!(lines[3].contains(",unread-1,"));
        assert!(lines[3].contains("\"Electric Attack Nanochip 3\",1,11.52,7.38,7,162,172,,false,"));
        assert!(lines[1].contains(",true,"));
    }

    #[test]
    fn a_capture_that_reads_no_queue_is_kept_with_why() {
        let temp = tempfile::tempdir().unwrap();
        let clock = Arc::new(MockClock::new(None, 0.0));
        let unavailable = Arc::new(SaleCaptureService::new(
            SaleCaptureProviders::default(),
            clock.clone(),
        ));
        let service = AuctionFeeResearchService::new(unavailable, clock, temp.path());
        service.start().unwrap();
        // Nothing was captured, so there are no pixels to keep: the sample
        // is not taken, and the reason is told.
        assert!(matches!(service.capture(), Err(ResearchError::Io(_))));
        let status = service.status();
        assert_eq!(status.sample_count, 0);
        assert!(status
            .last_capture
            .unwrap()
            .message
            .contains("reader is not available"));
    }

    #[test]
    fn start_is_idempotent_and_stop_preserves_the_completed_dataset_location() {
        let temp = tempfile::tempdir().unwrap();
        let service = service(temp.path());
        let first = service.start().unwrap();
        let second = service.start().unwrap();
        assert_eq!(first.output_dir, second.output_dir);
        let stopped = service.stop();
        assert!(!stopped.active);
        assert_eq!(stopped.output_dir, first.output_dir);
        assert!(matches!(service.capture(), Err(ResearchError::Inactive)));
    }

    #[test]
    fn stop_fences_an_in_flight_capture_from_the_next_session() {
        let temp = tempfile::tempdir().unwrap();
        let reached = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let service = service_with_gate(temp.path(), Some((reached.clone(), release.clone())));
        let first_dir = service.start().unwrap().output_dir.unwrap();
        let capture_service = service.clone();
        let capture = std::thread::spawn(move || capture_service.capture());

        reached.wait();
        service.stop();
        let second_dir = service.start().unwrap().output_dir.unwrap();
        release.wait();

        assert!(matches!(
            capture.join().unwrap(),
            Err(ResearchError::Inactive)
        ));
        assert!(!first_dir.join("sample-0001.png").exists());
        assert!(!second_dir.join("sample-0001.png").exists());
        let status = service.status();
        assert!(status.active);
        assert!(!status.busy);
        assert_eq!(status.sample_count, 0);
    }
}
