//! Reading a figure off one of the game's terminals: the Repair
//! Terminal's repair cost, for armour repair cost tracking, and the Trade
//! Terminal's total, for valuing limited armour.
//!
//! The whole game window is captured and the interface reader finds the
//! terminal by its title and the figure by its label, wherever the player
//! has put the window and at whatever size; the interface profile says
//! what the client prints. The answer is the figure exactly as the
//! terminal shows it, or a refusal that says why there is none. A figure
//! that did not read is never answered with a number: the player types it
//! instead, and a refusal tells them what to change for the next read.

use std::sync::Arc;

use crate::game_ui::field::{FieldError, Remembering};
use crate::game_ui::geometry::BgrImage;
use crate::game_ui::locate::{read_window_field, ReadError};
use crate::game_ui::profile::UiProfile;
use crate::game_ui::seams::{FrameReader, FrameUnavailable, GameWindowFrame};
use crate::game_ui::validate::without_unit;

/// The terminals a figure is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminal {
    /// The Repair Terminal, read for its repair cost.
    Repair,
    /// The Trade Terminal, read for the total of what is placed in it.
    Trade,
}

impl Terminal {
    /// The window's key in the interface profile.
    pub fn window(self) -> &'static str {
        match self {
            Terminal::Repair => "repair_terminal",
            Terminal::Trade => "trade_terminal",
        }
    }

    /// The figure's key among the window's fields in the profile.
    pub fn field(self) -> &'static str {
        match self {
            Terminal::Repair => "repair_cost",
            Terminal::Trade => "total",
        }
    }
}

/// A figure read off a terminal and validated.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalReading {
    /// The amount in PED.
    pub value_ped: f64,
    /// The amount exactly as the terminal prints it, without the unit:
    /// `2.20`, `1,234.56789`.
    pub text: String,
    /// What the recogniser answered, before validation: the evidence a
    /// recording keeps of where its figure came from.
    pub raw_text: String,
}

/// Why a terminal read produced no figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TerminalRefusal {
    #[error("the game window was not found")]
    GameWindowNotFound,
    /// The game window is wholly outside what the capturer can see (on
    /// another monitor than the one shared for capture, say).
    #[error("the game window is off the screen that can be captured")]
    GameWindowOffScreen,
    #[error("the game window could not be captured")]
    CaptureFailed,
    /// The terminal's title was not found: it is not open, its title is
    /// covered, or the title did not read.
    #[error("the terminal is not open, or its title is covered")]
    TerminalNotOpen,
    /// The terminal is open but its figure, or the label that marks it,
    /// was not found: another window covers it, or it is off the screen.
    #[error("the figure is covered or out of view")]
    FigureNotVisible,
    /// The figure is in view but its reading failed the field's grammar,
    /// is not as wide as the figure's ink, or did not read at all.
    #[error("the figure did not read as a valid amount")]
    FigureNotRead,
    /// No recogniser was loaded, or the interface profile does not
    /// describe the terminal. Nothing the player does in the game helps.
    #[error("the terminal reader is not available")]
    ReaderUnavailable,
}

impl From<FrameUnavailable> for TerminalRefusal {
    fn from(error: FrameUnavailable) -> Self {
        match error {
            FrameUnavailable::WindowNotFound => TerminalRefusal::GameWindowNotFound,
            FrameUnavailable::OutsideCapturableArea => TerminalRefusal::GameWindowOffScreen,
            // A terminal read is asked for by the player each time, so a
            // capture that was not allowed is simply one that failed; the
            // next read asks for consent again.
            FrameUnavailable::CaptureFailed | FrameUnavailable::CaptureNotStarted => {
                TerminalRefusal::CaptureFailed
            }
        }
    }
}

impl From<ReadError> for TerminalRefusal {
    fn from(error: ReadError) -> Self {
        match error {
            // The service hands the pipeline a recogniser that always
            // answers, so these mean the profile lacks the terminal.
            ReadError::NoRecogniser | ReadError::NotInProfile(_) => {
                TerminalRefusal::ReaderUnavailable
            }
            ReadError::WindowNotFound => TerminalRefusal::TerminalNotOpen,
            ReadError::Field(FieldError::LabelNotFound | FieldError::ValueNotFound) => {
                TerminalRefusal::FigureNotVisible
            }
            ReadError::Field(FieldError::Invalid { .. } | FieldError::WrongWidth { .. }) => {
                TerminalRefusal::FigureNotRead
            }
            ReadError::Field(FieldError::NoRecogniser) => TerminalRefusal::ReaderUnavailable,
        }
    }
}

/// The seams the composition root wires in.
pub struct RepairProviders {
    /// The whole game window.
    pub frame: GameWindowFrame,
    /// The interface profile; `None` when it could not be loaded.
    pub profile: Option<Arc<UiProfile>>,
    /// The recogniser; `None` when none was loaded. It may still fail on
    /// a particular crop, which is that crop not reading.
    pub read_text: Option<FrameReader>,
}

impl Default for RepairProviders {
    fn default() -> Self {
        Self {
            frame: Arc::new(|| Err(FrameUnavailable::WindowNotFound)),
            profile: None,
            read_text: None,
        }
    }
}

/// One-shot reads of the Repair Terminal's cost and the Trade
/// Terminal's total.
pub struct RepairOcrService {
    providers: RepairProviders,
}

impl RepairOcrService {
    pub fn new(providers: RepairProviders) -> Self {
        Self { providers }
    }

    /// The Repair Terminal's repair cost.
    pub fn scan_repair_cost(&self) -> Result<TerminalReading, TerminalRefusal> {
        self.read(Terminal::Repair)
    }

    /// The Trade Terminal's total.
    pub fn scan_trade_terminal_value(&self) -> Result<TerminalReading, TerminalRefusal> {
        self.read(Terminal::Trade)
    }

    /// Capture the game window and read `terminal`'s figure from it.
    /// Nothing is captured when there is no recogniser or profile to read
    /// it with, so the player is never sent to the game for a read that
    /// could not happen.
    pub fn read(&self, terminal: Terminal) -> Result<TerminalReading, TerminalRefusal> {
        let (Some(profile), Some(read_text)) =
            (self.providers.profile.as_deref(), &self.providers.read_text)
        else {
            return Err(TerminalRefusal::ReaderUnavailable);
        };
        let frame = (self.providers.frame)()?;

        // A crop the recogniser fails on reads as nothing: a title that
        // reads as nothing identifies no terminal, a figure that reads as
        // nothing fails its grammar.
        let recogniser =
            |image: &BgrImage| Some(read_text(image).unwrap_or_else(|| (String::new(), 0.0)));
        // Finding the terminal, its label and its figure can ask for the
        // same crop more than once; each is recognised once.
        let recogniser = Remembering::new(&recogniser);
        let read = read_window_field(
            &frame,
            profile,
            terminal.window(),
            terminal.field(),
            &recogniser,
        )?;
        // The profile gives these figures a numeric grammar; a reading
        // without a number cannot be an amount.
        let value_ped = read.parsed.number.ok_or(TerminalRefusal::FigureNotRead)?;
        Ok(TerminalReading {
            value_ped,
            text: without_unit(&read.parsed.text).to_string(),
            raw_text: read.raw,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_ui::geometry::Rect;
    use crate::game_ui::test_support::{blank, bundled_profile, draw_window, Lettering};

    const SCENE: [u8; 3] = [120, 110, 90];
    const FIGURE_HEIGHT: i32 = 14;

    fn profile() -> Arc<UiProfile> {
        Arc::new(bundled_profile())
    }

    /// A frame with one terminal open, its title, its label and its
    /// figure drawn as the client lays them out: the repair cost to the
    /// right of its label, the trade total on the line below its label.
    /// The figure's ink is as wide as `drawn` and reads as `reads`.
    fn terminal_frame(terminal: Terminal, drawn: &str, reads: &str) -> Lettering {
        let mut frame = Lettering::new(blank(1100, 700, SCENE));
        let window = Rect::new(100, 80, 700, 420);
        draw_window(&mut frame.image, window);
        let numerals = bundled_profile().read.numerals;
        match terminal {
            Terminal::Repair => {
                frame.text(Rect::new(180, 102, 400, 113), "REPAIR TERMINAL");
                frame.text(Rect::new(140, 250, 260, 261), "REPAIR COST:");
                frame.figure((330, 249), FIGURE_HEIGHT, drawn, reads, &numerals);
            }
            Terminal::Trade => {
                frame.text(Rect::new(180, 102, 400, 113), "TRADE TERMINAL");
                frame.text(Rect::new(140, 250, 270, 261), "TOTAL AMOUNT:");
                frame.figure((140, 290), FIGURE_HEIGHT, drawn, reads, &numerals);
            }
        }
        frame
    }

    fn service(frame: &Lettering) -> RepairOcrService {
        service_reading(frame, frame.recogniser())
    }

    /// The service over `frame`'s image, recognised by `read_text`.
    fn service_reading(
        frame: &Lettering,
        read_text: impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync + 'static,
    ) -> RepairOcrService {
        let image = frame.image.clone();
        RepairOcrService::new(RepairProviders {
            frame: Arc::new(move || Ok(image.clone())),
            profile: Some(profile()),
            read_text: Some(Arc::new(read_text)),
        })
    }

    #[test]
    fn the_repair_cost_is_read_exactly_as_the_terminal_shows_it() {
        let frame = terminal_frame(Terminal::Repair, "2.20 PED", "2.20 PED");
        let read = service(&frame).scan_repair_cost().expect("the cost reads");
        assert_eq!(read.value_ped, 2.2);
        assert_eq!(
            read.text, "2.20",
            "the amount as printed, trailing zero kept"
        );
        assert_eq!(read.raw_text, "2.20 PED");
    }

    #[test]
    fn the_trade_total_is_read_from_below_its_label() {
        let frame = terminal_frame(Terminal::Trade, "1,234.56789 PED", "1,234.56789 PED");
        let read = service(&frame)
            .scan_trade_terminal_value()
            .expect("the total reads");
        assert_eq!(read.value_ped, 1234.56789);
        assert_eq!(read.text, "1,234.56789");
    }

    #[test]
    fn a_closed_terminal_is_refused_even_with_the_other_one_open() {
        let trade = terminal_frame(Terminal::Trade, "12.50 PED", "12.50 PED");
        assert_eq!(
            service(&trade).scan_repair_cost(),
            Err(TerminalRefusal::TerminalNotOpen)
        );
        let repair = terminal_frame(Terminal::Repair, "2.20 PED", "2.20 PED");
        assert_eq!(
            service(&repair).scan_trade_terminal_value(),
            Err(TerminalRefusal::TerminalNotOpen)
        );
        let empty = Lettering::new(blank(1100, 700, SCENE));
        assert_eq!(
            service(&empty).scan_repair_cost(),
            Err(TerminalRefusal::TerminalNotOpen)
        );
    }

    #[test]
    fn each_way_of_having_no_frame_is_told_apart() {
        let recognised = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for (unavailable, refusal) in [
            (
                FrameUnavailable::WindowNotFound,
                TerminalRefusal::GameWindowNotFound,
            ),
            (
                FrameUnavailable::OutsideCapturableArea,
                TerminalRefusal::GameWindowOffScreen,
            ),
            (
                FrameUnavailable::CaptureFailed,
                TerminalRefusal::CaptureFailed,
            ),
            (
                FrameUnavailable::CaptureNotStarted,
                TerminalRefusal::CaptureFailed,
            ),
        ] {
            let counter = recognised.clone();
            let service = RepairOcrService::new(RepairProviders {
                frame: Arc::new(move || Err(unavailable)),
                profile: Some(profile()),
                read_text: Some(Arc::new(move |_| {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Some(("2.20 PED".into(), 0.99))
                })),
            });
            assert_eq!(service.scan_repair_cost(), Err(refusal));
            assert_eq!(service.scan_trade_terminal_value(), Err(refusal));
        }
        assert_eq!(
            recognised.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "nothing is recognised without a frame"
        );
    }

    #[test]
    fn a_figure_covered_by_another_window_is_refused() {
        let mut frame = terminal_frame(Terminal::Repair, "2.20 PED", "2.20 PED");
        // A window in front of the terminal's right half, over the
        // figure but clear of the title and the label.
        draw_window(&mut frame.image, Rect::new(300, 180, 950, 360));
        assert_eq!(
            service(&frame).scan_repair_cost(),
            Err(TerminalRefusal::FigureNotVisible)
        );
    }

    #[test]
    fn a_figure_the_grammar_refuses_is_a_refusal_not_a_number() {
        // A repeated digit collapsed: the grouping gives it away.
        let frame = terminal_frame(Terminal::Trade, "1,444.77 PED", "1,44.77 PED");
        assert_eq!(
            service(&frame).scan_trade_terminal_value(),
            Err(TerminalRefusal::FigureNotRead)
        );
        // No unit: not an amount of PED.
        let frame = terminal_frame(Terminal::Repair, "2.20 PED", "2.20");
        assert_eq!(
            service(&frame).scan_repair_cost(),
            Err(TerminalRefusal::FigureNotRead)
        );
    }

    #[test]
    fn a_figure_read_a_digit_short_of_its_ink_is_refused() {
        // Well formed, but the ink is a digit wider than the reading.
        let frame = terminal_frame(Terminal::Repair, "27.47542 PED", "27.4742 PED");
        assert_eq!(
            service(&frame).scan_repair_cost(),
            Err(TerminalRefusal::FigureNotRead)
        );
    }

    #[test]
    fn without_a_profile_or_a_recogniser_nothing_is_captured() {
        let frame = terminal_frame(Terminal::Repair, "2.20 PED", "2.20 PED");
        let captured = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let capturing = |captured: &Arc<std::sync::atomic::AtomicUsize>| -> GameWindowFrame {
            let counter = captured.clone();
            let image = frame.image.clone();
            Arc::new(move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(image.clone())
            })
        };
        let no_profile = RepairOcrService::new(RepairProviders {
            frame: capturing(&captured),
            profile: None,
            read_text: Some(Arc::new(frame.recogniser())),
        });
        let no_recogniser = RepairOcrService::new(RepairProviders {
            frame: capturing(&captured),
            profile: Some(profile()),
            read_text: None,
        });
        for service in [no_profile, no_recogniser] {
            assert_eq!(
                service.scan_repair_cost(),
                Err(TerminalRefusal::ReaderUnavailable)
            );
            assert_eq!(
                service.scan_trade_terminal_value(),
                Err(TerminalRefusal::ReaderUnavailable)
            );
        }
        assert_eq!(captured.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn a_crop_the_recogniser_fails_on_reads_as_nothing() {
        let frame = terminal_frame(Terminal::Repair, "2.20 PED", "2.20 PED");
        let failing_on = |text: &'static str| {
            let inner = frame.recogniser();
            move |image: &BgrImage| inner(image).filter(|(read, _)| read != text)
        };
        // The figure did not read: the figure is refused, not the reader.
        assert_eq!(
            service_reading(&frame, failing_on("2.20 PED")).scan_repair_cost(),
            Err(TerminalRefusal::FigureNotRead)
        );
        // The title did not read: no terminal is identified.
        assert_eq!(
            service_reading(&frame, failing_on("REPAIR TERMINAL")).scan_repair_cost(),
            Err(TerminalRefusal::TerminalNotOpen)
        );
    }
}
