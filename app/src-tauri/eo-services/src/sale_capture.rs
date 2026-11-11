//! Capturing the auction's sell queue, so that the listings the player is
//! about to make can be recorded from what the game shows rather than
//! retyped.
//!
//! The whole game window is captured and the sell queue is read from it
//! by [`read_sale_queue`], wherever the player has put the `AUCTION / SELL`
//! window and at whatever size. Every queued row the reader returns has
//! had each of its fields validated and, where the game prints a figure
//! twice or derives one from another, checked against the others; a row
//! that did not read whole is reported unread, with nothing of it kept.
//! A capture that could not read the queue at all answers why, so the
//! player knows what to change in the game for the next one.
//!
//! Each capture carries an identity, increasing in the order captures
//! were started, and the moment its screen was read. A caller that holds
//! drafts from one capture never mixes them with another's, and the
//! listings recorded from a capture are dated to when it was read.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::NaiveDateTime;

use crate::clock::Clock;
use crate::game_ui::geometry::BgrImage;
use crate::game_ui::profile::UiProfile;
use crate::game_ui::sale_queue::{
    read_sale_queue, ItemCatalogue, QueueCount, SaleQueue, SaleQueueError,
};
use crate::game_ui::seams::{FrameArea, FrameReader, FrameUnavailable, GameWindowFrame};

/// Why a capture produced no queue to draft listings from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SaleCaptureRefusal {
    #[error("the game window was not found")]
    GameWindowNotFound,
    /// The game window is wholly outside what the capturer can see.
    #[error("the game window is off the screen that can be captured")]
    GameWindowOffScreen,
    #[error("the game window could not be captured")]
    CaptureFailed,
    /// Screen capture was not allowed to start: the player declined, or
    /// was not asked for, consent to share the screen.
    #[error("screen capture was not allowed")]
    CaptureNotAllowed,
    /// The sell window's title was not found: the window is not open,
    /// its title is covered, or the title did not read.
    #[error("the sell window is not open, or its title is covered")]
    SellWindowNotOpen,
    /// The sell window is open but not wholly in view: a side is covered
    /// or off the screen, or the desktop scaled the game's picture.
    #[error("the sell window is not wholly in view")]
    SellWindowNotWhole,
    /// The queue's headings, rule lines or dividers are covered or did
    /// not read, so its columns cannot be told apart.
    #[error("the sell queue is covered or did not read")]
    QueueNotVisible,
    /// The toggle between prices in PED and as markup is covered or
    /// could not be read.
    #[error("the sell window's price toggle did not read")]
    PriceModeUnread,
    /// The window states prices as markup, not in PED.
    #[error("the sell window states prices as markup, not in PED")]
    OtherPriceMode,
    /// The queue's heading counts no queued item.
    #[error("the sell queue is empty")]
    QueueEmpty,
    /// No recogniser was loaded, or the interface profile does not
    /// describe the sell window. Nothing the player does in the game helps.
    #[error("the sell queue reader is not available")]
    ReaderUnavailable,
}

impl From<FrameUnavailable> for SaleCaptureRefusal {
    fn from(error: FrameUnavailable) -> Self {
        match error {
            FrameUnavailable::WindowNotFound => SaleCaptureRefusal::GameWindowNotFound,
            FrameUnavailable::OutsideCapturableArea => SaleCaptureRefusal::GameWindowOffScreen,
            FrameUnavailable::CaptureFailed => SaleCaptureRefusal::CaptureFailed,
            FrameUnavailable::CaptureNotStarted => SaleCaptureRefusal::CaptureNotAllowed,
        }
    }
}

impl From<SaleQueueError> for SaleCaptureRefusal {
    fn from(error: SaleQueueError) -> Self {
        match error {
            // The service hands the reader a recogniser that always
            // answers, so this means the profile lacks the sell window.
            SaleQueueError::NoRecogniser | SaleQueueError::NotInProfile(_) => {
                SaleCaptureRefusal::ReaderUnavailable
            }
            SaleQueueError::WindowNotFound => SaleCaptureRefusal::SellWindowNotOpen,
            SaleQueueError::ExtentUncertain | SaleQueueError::WindowClipped => {
                SaleCaptureRefusal::SellWindowNotWhole
            }
            SaleQueueError::HeadingNotFound(_)
            | SaleQueueError::ColumnsMerged
            | SaleQueueError::TableNotFound => SaleCaptureRefusal::QueueNotVisible,
            SaleQueueError::ModeUnread => SaleCaptureRefusal::PriceModeUnread,
            SaleQueueError::OtherMode => SaleCaptureRefusal::OtherPriceMode,
        }
    }
}

/// The queue as one capture read it.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueReading {
    /// When the screen was read: what a listing recorded from it is dated
    /// to.
    pub read_at: NaiveDateTime,
    pub queue: SaleQueue,
}

/// One capture of the sell queue: its identity, and the queue it read or
/// why there is none.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueCapture {
    /// Increasing in the order captures were started, from 1.
    pub id: u64,
    pub outcome: Result<QueueReading, SaleCaptureRefusal>,
}

/// The seams the composition root wires in.
pub struct SaleCaptureProviders {
    /// The whole game window.
    pub frame: GameWindowFrame,
    /// The interface profile; `None` when it could not be loaded.
    pub profile: Option<Arc<UiProfile>>,
    /// The items the application knows, which confirm a queued name.
    pub catalogue: Arc<ItemCatalogue>,
    /// The recogniser; `None` when none was loaded. It may still fail on
    /// a particular crop, which is that crop not reading.
    pub read_text: Option<FrameReader>,
}

impl Default for SaleCaptureProviders {
    fn default() -> Self {
        Self {
            frame: Arc::new(|_| Err(FrameUnavailable::WindowNotFound)),
            profile: None,
            catalogue: Arc::new(ItemCatalogue::default()),
            read_text: None,
        }
    }
}

/// One-shot captures of the auction's sell queue.
pub struct SaleCaptureService {
    providers: SaleCaptureProviders,
    clock: Arc<dyn Clock>,
    /// The identity the next capture takes.
    next: AtomicU64,
}

impl SaleCaptureService {
    pub fn new(providers: SaleCaptureProviders, clock: Arc<dyn Clock>) -> Self {
        Self {
            providers,
            clock,
            next: AtomicU64::new(1),
        }
    }

    /// Capture the game window and read the sell queue from it.
    pub fn capture(&self) -> QueueCapture {
        self.observe().0
    }

    /// Capture and read the sell queue, keeping the captured frame: the
    /// evidence a development recording keeps of what was read. The frame
    /// is `None` when nothing was captured.
    ///
    /// Nothing is captured when there is no recogniser or profile to read
    /// with, so the player is never asked to share the screen for a read
    /// that could not happen.
    pub fn observe(&self) -> (QueueCapture, Option<BgrImage>) {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let refused = |refusal: SaleCaptureRefusal| QueueCapture {
            id,
            outcome: Err(refusal),
        };
        let (Some(profile), Some(read_text)) =
            (self.providers.profile.as_deref(), &self.providers.read_text)
        else {
            return (refused(SaleCaptureRefusal::ReaderUnavailable), None);
        };
        let image = match (self.providers.frame)(FrameArea::Whole) {
            Ok(frame) => frame.image,
            Err(unavailable) => return (refused(unavailable.into()), None),
        };
        // The moment the pixels were taken, not when reading them ends: a
        // read takes seconds, and the listing is dated to the screen.
        let read_at = self.clock.now();

        // A crop the recogniser fails on reads as nothing: a title that
        // reads as nothing identifies no window, a figure that reads as
        // nothing fails its grammar.
        let recogniser =
            |crop: &BgrImage| Some(read_text(crop).unwrap_or_else(|| (String::new(), 0.0)));
        let outcome = match read_sale_queue(&image, profile, &self.providers.catalogue, &recogniser)
        {
            Ok(queue) if queue.count == QueueCount::Complete(0) => {
                Err(SaleCaptureRefusal::QueueEmpty)
            }
            Ok(queue) => Ok(QueueReading { read_at, queue }),
            Err(error) => Err(error.into()),
        };
        (QueueCapture { id, outcome }, Some(image))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    use crate::clock::MockClock;
    use crate::game_ui::geometry::Rect;
    use crate::game_ui::sale_queue::{Buyout, ItemName, QueueField, RowRefusal};
    use crate::game_ui::test_support::{
        bundled_profile, draw_window, sale_scene, sale_window, standard_sale_layout, window_frame,
        Lettering, SaleLayout,
    };

    fn moment() -> NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, 11, 3)
            .unwrap()
            .and_hms_opt(19, 42, 7)
            .unwrap()
    }

    fn catalogue() -> Arc<ItemCatalogue> {
        Arc::new(ItemCatalogue::new([(
            "Electric Attack Nanochip 3".to_string(),
            Some(39.0),
        )]))
    }

    /// The service over `image`, recognised by `read_text`.
    fn service_over(
        image: &BgrImage,
        read_text: impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync + 'static,
    ) -> SaleCaptureService {
        let image = image.clone();
        SaleCaptureService::new(
            SaleCaptureProviders {
                frame: Arc::new(move |area| window_frame(&image, (0, 0), area)),
                profile: Some(Arc::new(bundled_profile())),
                catalogue: catalogue(),
                read_text: Some(Arc::new(read_text)),
            },
            Arc::new(MockClock::new(Some(moment()), 0.0)),
        )
    }

    fn service(drawn: &Lettering) -> SaleCaptureService {
        service_over(&drawn.image, drawn.recogniser())
    }

    /// The service over `layout`, its recogniser answering `reads` for a
    /// crop the drawn text is `drawn`.
    fn misreading(layout: &SaleLayout, drawn: &'static str, reads: &'static str) -> QueueCapture {
        let d = sale_window(layout);
        let inner = d.recogniser();
        service_over(&d.image, move |crop| {
            inner(crop).map(|(text, c)| {
                (
                    if text == drawn {
                        reads.to_string()
                    } else {
                        text
                    },
                    c,
                )
            })
        })
        .capture()
    }

    fn reading(capture: QueueCapture) -> QueueReading {
        capture.outcome.expect("the queue reads")
    }

    #[test]
    fn a_whole_queue_is_read_with_the_moment_its_screen_was_taken() {
        let capture = service(&sale_window(&standard_sale_layout())).capture();
        assert_eq!(capture.id, 1);
        let read = reading(capture);
        assert_eq!(read.read_at, moment());
        assert_eq!(read.queue.count, QueueCount::Complete(3));
        let rows: Vec<(&ItemName, &str, &str, &Buyout)> = read
            .queue
            .rows
            .iter()
            .map(|row| {
                (
                    &row.name,
                    row.opening_bid.text.as_str(),
                    row.fee.text.as_str(),
                    &row.buyout,
                )
            })
            .collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0].0,
            &ItemName::Catalogued("Electric Attack Nanochip 3".into())
        );
        assert_eq!((rows[0].1, rows[0].2), ("162", "7.33 PED"));
        assert_eq!(rows[1].3, &Buyout::Off);
        assert_eq!(
            rows[2].0,
            &ItemName::Unconfirmed("Skyripper Loyalist (L)".into())
        );
    }

    #[test]
    fn each_capture_takes_the_next_identity_whatever_it_answers() {
        let drawn = sale_window(&standard_sale_layout());
        let service = service(&drawn);
        assert_eq!(service.capture().id, 1);
        assert_eq!(service.capture().id, 2);
        let unavailable = SaleCaptureService::new(
            SaleCaptureProviders::default(),
            Arc::new(MockClock::new(Some(moment()), 0.0)),
        );
        let first = unavailable.capture();
        assert_eq!(
            (first.id, first.outcome),
            (1, Err(SaleCaptureRefusal::ReaderUnavailable))
        );
        assert_eq!(unavailable.capture().id, 2);
    }

    #[test]
    fn short_long_and_wrong_readings_leave_their_rows_unread_with_no_figure() {
        for (drawn, reads, field) in [
            // A digit short of the bid's ink.
            ("162", "16", QueueField::OpeningBid),
            // A digit more than the fee's ink.
            ("7.33 PED", "7.333 PED", QueueField::Fee),
        ] {
            let read = reading(misreading(&standard_sale_layout(), drawn, reads));
            assert_eq!(read.queue.rows.len(), 2, "{drawn} read as {reads}");
            assert_eq!(read.queue.unread.len(), 1);
            assert_eq!(
                read.queue.unread[0].refusals,
                vec![RowRefusal::WrongWidth {
                    field,
                    raw: reads.into()
                }]
            );
            assert!(read
                .queue
                .rows
                .iter()
                .all(|row| row.opening_bid.text != "16" && row.fee.text != "7.333 PED"));
        }
        // A wrong digit of the same width: the buyout's own figure
        // disagrees with it, so the row is unread and the price kept
        // nowhere.
        let read = reading(misreading(&standard_sale_layout(), "172", "173"));
        assert_eq!(read.queue.rows.len(), 2);
        assert!(read
            .queue
            .rows
            .iter()
            .all(|row| !matches!(&row.buyout, Buyout::Price(p) if p.value == 173)));
        assert!(matches!(
            read.queue.count,
            QueueCount::Incomplete {
                heading: 3,
                read: 2,
                unread: 1,
                partial: 0
            }
        ));
    }

    #[test]
    fn an_empty_queue_is_refused_as_empty() {
        let mut layout = standard_sale_layout();
        layout.rows.clear();
        layout.heading = ("Sell Queue", "0");
        assert_eq!(
            service(&sale_window(&layout)).capture().outcome,
            Err(SaleCaptureRefusal::QueueEmpty)
        );
    }

    #[test]
    fn a_closed_or_unreadable_sell_window_is_refused_with_why() {
        let mut other = Lettering::new(sale_scene());
        draw_window(&mut other.image, Rect::new(20, 20, 900, 600));
        other.text(Rect::new(100, 42, 300, 53), "INVENTORY");
        assert_eq!(
            service(&other).capture().outcome,
            Err(SaleCaptureRefusal::SellWindowNotOpen)
        );

        let mut clipped = standard_sale_layout();
        clipped.window = Rect::new(120, 20, 1480, 800);
        assert_eq!(
            service(&sale_window(&clipped)).capture().outcome,
            Err(SaleCaptureRefusal::SellWindowNotWhole)
        );

        let mut markup = standard_sale_layout();
        markup.markup_mode = true;
        assert_eq!(
            service(&sale_window(&markup)).capture().outcome,
            Err(SaleCaptureRefusal::OtherPriceMode)
        );

        let mut merged = standard_sale_layout();
        merged.hidden_dividers = vec![2];
        assert_eq!(
            service(&sale_window(&merged)).capture().outcome,
            Err(SaleCaptureRefusal::QueueNotVisible)
        );

        assert_eq!(
            misreading(&standard_sale_layout(), "PED", "").outcome,
            Err(SaleCaptureRefusal::PriceModeUnread)
        );
    }

    #[test]
    fn a_crop_the_recogniser_fails_on_reads_as_nothing() {
        let d = sale_window(&standard_sale_layout());
        let failing_on = |text: &'static str| {
            let inner = d.recogniser();
            move |image: &BgrImage| inner(image).filter(|(read, _)| read != text)
        };
        // The title did not read: no sell window is identified.
        assert_eq!(
            service_over(&d.image, failing_on("AUCTION / SELL"))
                .capture()
                .outcome,
            Err(SaleCaptureRefusal::SellWindowNotOpen)
        );
        // A fee did not read: its row is unread, the reader is not.
        let read = reading(service_over(&d.image, failing_on("7.33 PED")).capture());
        assert_eq!(read.queue.rows.len(), 2);
        assert_eq!(
            read.queue.unread[0].refusals,
            vec![RowRefusal::Invalid {
                field: QueueField::Fee,
                raw: String::new()
            }]
        );
    }

    #[test]
    fn each_way_of_having_no_frame_is_told_apart_and_nothing_is_recognised() {
        let recognised = Arc::new(AtomicUsize::new(0));
        for (unavailable, refusal) in [
            (
                FrameUnavailable::WindowNotFound,
                SaleCaptureRefusal::GameWindowNotFound,
            ),
            (
                FrameUnavailable::OutsideCapturableArea,
                SaleCaptureRefusal::GameWindowOffScreen,
            ),
            (
                FrameUnavailable::CaptureFailed,
                SaleCaptureRefusal::CaptureFailed,
            ),
            (
                FrameUnavailable::CaptureNotStarted,
                SaleCaptureRefusal::CaptureNotAllowed,
            ),
        ] {
            let counter = recognised.clone();
            let service = SaleCaptureService::new(
                SaleCaptureProviders {
                    frame: Arc::new(move |_| Err(unavailable)),
                    profile: Some(Arc::new(bundled_profile())),
                    catalogue: catalogue(),
                    read_text: Some(Arc::new(move |_| {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Some(("162".into(), 0.99))
                    })),
                },
                Arc::new(MockClock::new(Some(moment()), 0.0)),
            );
            let (capture, frame) = service.observe();
            assert_eq!(capture.outcome, Err(refusal));
            assert!(frame.is_none());
        }
        assert_eq!(recognised.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn without_a_profile_or_a_recogniser_nothing_is_captured() {
        let d = sale_window(&standard_sale_layout());
        let captured = Arc::new(AtomicUsize::new(0));
        let capturing = || -> GameWindowFrame {
            let counter = captured.clone();
            let image = d.image.clone();
            Arc::new(move |area| {
                counter.fetch_add(1, Ordering::SeqCst);
                window_frame(&image, (0, 0), area)
            })
        };
        for (profile, read_text) in [
            (None, Some(Arc::new(d.recogniser()) as FrameReader)),
            (Some(Arc::new(bundled_profile())), None),
        ] {
            let service = SaleCaptureService::new(
                SaleCaptureProviders {
                    frame: capturing(),
                    profile,
                    catalogue: catalogue(),
                    read_text,
                },
                Arc::new(MockClock::new(Some(moment()), 0.0)),
            );
            let (capture, frame) = service.observe();
            assert_eq!(capture.outcome, Err(SaleCaptureRefusal::ReaderUnavailable));
            assert!(frame.is_none());
        }
        assert_eq!(captured.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn an_observation_keeps_the_frame_it_read() {
        let d = sale_window(&standard_sale_layout());
        let (capture, frame) = service(&d).observe();
        assert!(capture.outcome.is_ok());
        assert_eq!(frame, Some(d.image.clone()));
    }
}
