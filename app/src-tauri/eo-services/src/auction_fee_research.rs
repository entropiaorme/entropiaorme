//! Development-only auction-fee sampling.
//!
//! A live research session turns successive reads of the game's sale window
//! into a reproducible dataset: one PNG preserving the source pixels, one
//! versioned JSONL observation, and one analysis-friendly CSV row per screen
//! grab. It never creates a listing or writes accounting state.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::clock::Clock;
use crate::sale_window_ocr::SaleWindowOcrService;
use crate::screen_capture::frame_to_png;

const SCHEMA_VERSION: u32 = 1;
const STUDY_DIR: &str = "auction-fee-study";
const CSV_HEADER: &str = "schema_version,sample,captured_at_local,item_name,quantity,tt_value,auction_fee,auction_days,starting_bid,buyout,confidence,accepted,error,unread,screenshot,sha256\n";
const COLLECTION_PROTOCOL: &str = r#"# Auction fee capture protocol

Do not submit a listing. This session only reads the quote shown by the sale window.

1. Dock the sale window in the calibrated bottom-right position at the default interface scale.
2. Change one input at a time, wait for the quoted fee to settle, then press Space once.
3. Build a price sweep for one fixed stack: sample densely around the 0.50 PED minimum-fee knee, then at progressively wider starting bids.
4. Repeat price sweeps across stacks near 0.10, 1, 5, 10, 50, 100, and 500 PED TT where held items permit.
5. At one fixed TT and starting bid, vary duration across 1, 7, and 14 days. At another fixed point, vary buyout while leaving the starting bid unchanged.
6. Where practical, capture the same TT and bid using different item identities or quantities.
7. Repeat at least ten unchanged scenarios. Exact repeats reveal quote or OCR noise.

Aim for 60 to 100 accepted samples. Unread samples remain useful as OCR evidence but do not count towards that target. `samples.csv` is the modelling table; `samples.jsonl` preserves the complete structured read; each row names and hashes its source PNG.
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
    region: Option<Value>,
    read: Value,
}

/// One actor-owned research session over the production sale-window reader.
pub struct AuctionFeeResearchService {
    sale_window: Arc<SaleWindowOcrService>,
    clock: Arc<dyn Clock>,
    root: PathBuf,
    state: Mutex<State>,
}

impl AuctionFeeResearchService {
    pub fn new(
        sale_window: Arc<SaleWindowOcrService>,
        clock: Arc<dyn Clock>,
        data_dir: &Path,
    ) -> Arc<Self> {
        Arc::new(Self {
            sale_window,
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
        let observation = self.sale_window.observe_sale_window();
        let frame = observation.frame.ok_or_else(|| {
            ResearchError::Io(std::io::Error::other(
                observation
                    .read
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("sale window capture failed"),
            ))
        })?;
        let png = frame_to_png(&frame).ok_or(ResearchError::Encode)?;
        let screenshot = format!("sample-{sample:04}.png");
        let sha256 = format!("{:x}", Sha256::digest(&png));
        let captured_at = self.clock.now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
        let read = &observation.read;
        let accepted = ["tt_value", "auction_fee", "starting_bid"]
            .iter()
            .all(|field| read.get(field).and_then(Value::as_f64).is_some());
        let message = if accepted {
            format!("Sample {sample} saved")
        } else {
            format!("Sample {sample} saved with unread modelling fields")
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
            region: observation.region,
            read: observation.read,
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
            "region": prepared.region,
            "read": prepared.read,
        });
        append_line(
            &output_dir.join("samples.jsonl"),
            &serde_json::to_string(&record).expect("research record serialises"),
        )?;
        append_line(
            &output_dir.join("samples.csv"),
            &csv_row(
                sample,
                &prepared.captured_at,
                prepared.capture.accepted,
                &prepared.screenshot,
                &prepared.sha256,
                &prepared.read,
            ),
        )?;
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

fn csv_row(
    sample: u64,
    captured_at: &str,
    accepted: bool,
    screenshot: &str,
    sha256: &str,
    read: &Value,
) -> String {
    let string = |key: &str| read.get(key).and_then(Value::as_str).unwrap_or("");
    let number = |key: &str| {
        read.get(key)
            .and_then(Value::as_f64)
            .map(|value| value.to_string())
            .unwrap_or_default()
    };
    let unread = read
        .get("unread")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("|")
        })
        .unwrap_or_default();
    [
        SCHEMA_VERSION.to_string(),
        sample.to_string(),
        csv_cell(captured_at),
        csv_cell(string("item_name")),
        number("quantity"),
        number("tt_value"),
        number("auction_fee"),
        number("auction_days"),
        number("starting_bid"),
        number("buyout"),
        number("confidence"),
        accepted.to_string(),
        csv_cell(string("error")),
        csv_cell(&unread),
        csv_cell(screenshot),
        sha256.to_string(),
    ]
    .join(",")
}

fn csv_cell(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::MockClock;
    use crate::game_ui::geometry::BgrImage;
    use crate::sale_window_ocr::{SaleWindowProviders, SALE_FIELDS};
    use crate::scan_presets::{CellGeometry, PanelAnchor};
    use std::sync::Barrier;

    fn service(root: &Path) -> Arc<AuctionFeeResearchService> {
        service_with_gate(root, None)
    }

    fn service_with_gate(
        root: &Path,
        gate: Option<(Arc<Barrier>, Arc<Barrier>)>,
    ) -> Arc<AuctionFeeResearchService> {
        let cells: Vec<(String, CellGeometry)> = SALE_FIELDS
            .iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    (*name).to_string(),
                    CellGeometry {
                        x_left: 0,
                        x_right: 4,
                        first_y_top: index as i64 * 4,
                        last_y_top: index as i64 * 4,
                        height: 4,
                    },
                )
            })
            .collect();
        let answers = Arc::new(Mutex::new(vec![
            "Animal Oil Residue",
            "100",
            "12.5",
            "0.71",
            "7",
            "15",
            "16",
        ]));
        let sale_window = Arc::new(SaleWindowOcrService::new(SaleWindowProviders {
            sale_window_region: Arc::new(|| Some(([10, 20], [50, 60]))),
            anchor: Arc::new(move || PanelAnchor {
                width: 40,
                height: 40,
                right_offset: 0,
                bottom_offset: 0,
                n_rows: None,
                cells: cells.clone(),
            }),
            capture_region: Arc::new(move |_, _, _, _| {
                if let Some((reached, release)) = &gate {
                    reached.wait();
                    release.wait();
                }
                Some(BgrImage {
                    data: vec![7; 40 * 40 * 3],
                    h: 40,
                    w: 40,
                })
            }),
            read_text: Arc::new(move |_| {
                let mut answers = answers.lock().unwrap();
                Some((answers.remove(0).to_string(), 0.99))
            }),
        }));
        AuctionFeeResearchService::new(sale_window, Arc::new(MockClock::new(None, 0.0)), root)
    }

    #[test]
    fn a_capture_writes_a_hashed_evidence_row_without_mutating_any_domain_state() {
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
            .contains("60 to 100 accepted samples"));
        let jsonl = std::fs::read_to_string(dir.join("samples.jsonl")).unwrap();
        let row: Value = serde_json::from_str(jsonl.trim()).unwrap();
        assert_eq!(row["read"]["tt_value"], 12.5);
        assert_eq!(row["read"]["auction_fee"], 0.71);
        assert_eq!(row["accepted"], true);
        assert_eq!(row["sha256"].as_str().unwrap().len(), 64);
        let csv = std::fs::read_to_string(dir.join("samples.csv")).unwrap();
        assert!(csv.contains("Animal Oil Residue"));
        assert!(csv.contains(",12.5,0.71,7,15,16,"));
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
