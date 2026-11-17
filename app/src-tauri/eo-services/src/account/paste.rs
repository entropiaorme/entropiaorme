//! Read the account site's inventory as its Copy CSV emits it, into the
//! same holdings a reading keeps, so a pasted inventory and a reading of
//! the same inventory are the same thing to whatever reads them.
//!
//! The export is a header row (`Name`, `Quantity`, `Value(PED)`,
//! `Container`, `ContainerRefId`) and one row per row of the site's list,
//! tab-separated (a comma is accepted where the header uses commas). A
//! field may be quoted, and one storage container's name carries a
//! trailing newline the export does not quote, so a row that ends early
//! continues on the next line.
//!
//! Exact or refused, as a reading is: a row that does not read refuses the
//! whole paste, naming its line and why, because one row left out would
//! read as stock gone.

use std::collections::BTreeMap;

use super::amount::{pec_from_decimal, ValueRefusal, MAX_QUANTITY};
use super::reading::{Container, Holding, HoldingKey, Row, RowRefusal};

/// Why one row of a paste does not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteRowFault {
    /// Fewer fields than the header names.
    Columns,
    /// More fields than the header names.
    ExtraFields,
    BlankName,
    /// The quantity is not a whole number of units the app reads.
    Quantity,
    Value(ValueRefusal),
    Container(RowRefusal),
}

impl PasteRowFault {
    pub fn describe(self) -> &'static str {
        match self {
            PasteRowFault::Columns => "has fewer fields than the header",
            PasteRowFault::ExtraFields => "has more fields than the header",
            PasteRowFault::BlankName => "has no name",
            PasteRowFault::Quantity => "has a quantity that is not a whole number of units",
            PasteRowFault::Value(refusal) => refusal.describe(),
            PasteRowFault::Container(refusal) => refusal.describe(),
        }
    }
}

/// Why a paste does not read as an inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteRefusal {
    /// No header naming the name, quantity, value and container columns:
    /// not the inventory's Copy CSV.
    NoHeader,
    /// A header and no rows.
    NoRows,
    /// One row does not read (1-based line of the paste).
    Row { line: usize, fault: PasteRowFault },
    /// Totals too large to add up.
    TooLarge,
}

impl PasteRefusal {
    pub fn describe(self) -> String {
        match self {
            PasteRefusal::NoHeader => "the paste is not the inventory's Copy CSV: its header \
                 names no Name, Quantity, Value and Container columns"
                .to_string(),
            PasteRefusal::NoRows => "the paste holds a header and no items".to_string(),
            PasteRefusal::Row { line, fault } => format!("line {line} {}", fault.describe()),
            PasteRefusal::TooLarge => {
                "the paste's totals are beyond what the app counts in".to_string()
            }
        }
    }
}

/// One record of the paste: its fields and the line it starts on.
struct Record {
    line: usize,
    fields: Vec<String>,
}

/// Split text into records of fields, honouring quotes (a doubled quote is
/// a quote; a newline inside quotes stays in the field).
fn records(text: &str, separator: char) -> Vec<Record> {
    let mut out = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut line = 1usize;
    let mut start = 1usize;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            }
            '"' if field.trim().is_empty() => {
                field.clear();
                quoted = true;
            }
            '\n' => {
                line += 1;
                if quoted {
                    field.push('\n');
                } else {
                    fields.push(std::mem::take(&mut field));
                    out.push(Record {
                        line: start,
                        fields: std::mem::take(&mut fields),
                    });
                    start = line;
                }
            }
            '\r' if !quoted => {}
            ch if ch == separator && !quoted => fields.push(std::mem::take(&mut field)),
            ch => field.push(ch),
        }
    }
    if !field.is_empty() || !fields.is_empty() {
        fields.push(field);
        out.push(Record {
            line: start,
            fields,
        });
    }
    out
}

/// A container field cut at the newline one storage name carries: it opens
/// `STORAGE (` and does not close.
fn is_split_storage(field: &str) -> bool {
    let field = field.trim_start();
    field.starts_with("STORAGE (") && !field.contains(')')
}

fn is_blank(record: &Record) -> bool {
    record.fields.iter().all(|field| field.trim().is_empty())
}

/// Where each needed column sits.
struct Columns {
    count: usize,
    name: usize,
    quantity: usize,
    value: usize,
    container: usize,
    parent: Option<usize>,
}

fn columns(header: &[String]) -> Option<Columns> {
    let find = |wanted: &dyn Fn(&str) -> bool| {
        header
            .iter()
            .position(|field| wanted(&field.trim().to_ascii_lowercase()))
    };
    Some(Columns {
        count: header.len(),
        name: find(&|field| field == "name")?,
        quantity: find(&|field| field == "quantity")?,
        value: find(&|field| field.starts_with("value"))?,
        container: find(&|field| field == "container")?,
        parent: find(&|field| field == "containerrefid"),
    })
}

fn quantity(text: &str) -> Option<i64> {
    text.trim()
        .parse::<i64>()
        .ok()
        .filter(|quantity| (0..=MAX_QUANTITY).contains(quantity))
}

fn value(text: &str) -> Result<super::amount::Pec, ValueRefusal> {
    let number = serde_json::from_str::<serde_json::Number>(&text.trim().replace(',', "."))
        .map_err(|_| ValueRefusal::Unreadable)?;
    pec_from_decimal(&number)
}

/// Read a pasted inventory into holdings, keyed as a reading keys them.
pub fn holdings_from_paste(text: &str) -> Result<BTreeMap<HoldingKey, Holding>, PasteRefusal> {
    let first = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or(PasteRefusal::NoHeader)?;
    let separator = if first.contains('\t') { '\t' } else { ',' };
    let mut records = records(text, separator).into_iter();
    let header = records
        .by_ref()
        .find(|record| !is_blank(record))
        .ok_or(PasteRefusal::NoHeader)?;
    let columns = columns(&header.fields).ok_or(PasteRefusal::NoHeader)?;

    let mut rows: BTreeMap<HoldingKey, Vec<Row>> = BTreeMap::new();
    let mut any = false;
    while let Some(mut record) = records.next() {
        if is_blank(&record) {
            continue;
        }
        // A row continues on the next line only where the export split a
        // storage container's name at the newline it carries: its container
        // field opens `STORAGE (` and does not close.
        while record.fields.len() < columns.count
            && record.fields.len() == columns.container + 1
            && is_split_storage(&record.fields[columns.container])
        {
            let Some(next) = records.next() else {
                break;
            };
            let mut rest = next.fields.into_iter();
            if let (Some(last), Some(first)) = (record.fields.last_mut(), rest.next()) {
                last.push('\n');
                last.push_str(&first);
            }
            record.fields.extend(rest);
        }
        // A trailing ContainerRefId left empty may be stripped on the way.
        if record.fields.len() + 1 == columns.count
            && columns.parent == Some(columns.count - 1)
            && !record
                .fields
                .last()
                .is_some_and(|last| is_split_storage(last))
        {
            record.fields.push(String::new());
        }
        let refuse = |fault| PasteRefusal::Row {
            line: record.line,
            fault,
        };
        if record.fields.len() < columns.count {
            return Err(refuse(PasteRowFault::Columns));
        }
        if record.fields.len() > columns.count {
            return Err(refuse(PasteRowFault::ExtraFields));
        }
        let field = |index: usize| record.fields[index].as_str();
        let name = field(columns.name).trim();
        if name.is_empty() {
            return Err(refuse(PasteRowFault::BlankName));
        }
        let units = quantity(field(columns.quantity)).ok_or(refuse(PasteRowFault::Quantity))?;
        let pec =
            value(field(columns.value)).map_err(|fault| refuse(PasteRowFault::Value(fault)))?;
        let parent = columns
            .parent
            .map(|index| field(index).trim())
            .filter(|parent| !parent.is_empty() && !parent.eq_ignore_ascii_case("null"));
        let container = match parent {
            Some(_) => {
                let parent = field(columns.container).trim();
                if parent.is_empty() {
                    return Err(refuse(PasteRowFault::Container(RowRefusal::BlankContainer)));
                }
                Container::Attached {
                    parent: parent.to_string(),
                }
            }
            None => Container::from_site(field(columns.container))
                .map_err(|fault| refuse(PasteRowFault::Container(fault)))?,
        };
        rows.entry(HoldingKey {
            name: name.to_string(),
            container,
        })
        .or_default()
        .push(Row {
            quantity: units,
            value: pec,
        });
        any = true;
    }
    if !any {
        return Err(PasteRefusal::NoRows);
    }
    rows.into_iter()
        .map(|(key, rows)| {
            Holding::new(rows)
                .map(|holding| (key, holding))
                .map_err(|_| PasteRefusal::TooLarge)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "Name\tQuantity\tValue(PED)\tContainer\tContainerRefId\n";

    fn held(holdings: &BTreeMap<HoldingKey, Holding>, name: &str, container: Container) -> i64 {
        holdings
            .get(&HoldingKey {
                name: name.to_string(),
                container,
            })
            .map_or(0, Holding::quantity)
    }

    #[test]
    fn reads_each_place_and_an_attached_row_as_a_reading_does() {
        let paste = format!(
            "{HEADER}Shrapnel\t40000\t4.0000\tCARRIED\t\n\
             Shrapnel\t5000\t0.5000\tSTORAGE (Calypso)\t\n\
             Armor Plating Mk. 6A\t1\t25.0000\tPaneleon Arm Guards (M)\t7\n\
             Gorgon Gloves (M)\t1\t1.4000\tAUCTION\tnull\n"
        );
        let holdings = holdings_from_paste(&paste).unwrap();
        assert_eq!(held(&holdings, "Shrapnel", Container::Carried), 40_000);
        assert_eq!(
            held(
                &holdings,
                "Shrapnel",
                Container::Storage {
                    planet: "Calypso".to_string()
                }
            ),
            5_000
        );
        assert_eq!(
            held(
                &holdings,
                "Armor Plating Mk. 6A",
                Container::Attached {
                    parent: "Paneleon Arm Guards (M)".to_string()
                }
            ),
            1
        );
        assert_eq!(held(&holdings, "Gorgon Gloves (M)", Container::Auction), 1);
    }

    #[test]
    fn a_storage_name_with_a_trailing_newline_continues_on_the_next_line() {
        let paste = format!("{HEADER}Common Dung\t29\t0.0000\tSTORAGE (The Hub\n)\t\n");
        let holdings = holdings_from_paste(&paste).unwrap();
        assert_eq!(
            held(
                &holdings,
                "Common Dung",
                Container::Storage {
                    planet: "The Hub".to_string()
                }
            ),
            29
        );
    }

    #[test]
    fn a_stripped_empty_trailing_field_is_read_as_empty() {
        let paste =
            format!("{HEADER}Iron Stone\t5\t0.6500\tCARRIED\nShrapnel\t100\t0.0100\tCARRIED\t\n");
        let holdings = holdings_from_paste(&paste).unwrap();
        assert_eq!(held(&holdings, "Iron Stone", Container::Carried), 5);
        assert_eq!(held(&holdings, "Shrapnel", Container::Carried), 100);
    }

    #[test]
    fn a_short_row_is_never_joined_to_the_next_unless_a_storage_name_was_split() {
        // Two fields short: not a stripped trailing field, nor a split name.
        let short = format!("{HEADER}Iron Stone\t5\t0.6500\nCARRIED\t\t\n");
        assert_eq!(
            holdings_from_paste(&short),
            Err(PasteRefusal::Row {
                line: 2,
                fault: PasteRowFault::Columns
            })
        );
        let extra = format!("{HEADER}Iron Stone\t5\t0.6500\tCARRIED\t\tmore\n");
        assert_eq!(
            holdings_from_paste(&extra),
            Err(PasteRefusal::Row {
                line: 2,
                fault: PasteRowFault::ExtraFields
            })
        );
    }

    #[test]
    fn a_quoted_field_keeps_its_separator_and_newline() {
        let paste = "Name,Quantity,Value(PED),Container,ContainerRefId\n\
             \"Pixie, Glowing\",3,0.30,\"STORAGE (The Hub\n)\",\n";
        let holdings = holdings_from_paste(paste).unwrap();
        assert_eq!(
            held(
                &holdings,
                "Pixie, Glowing",
                Container::Storage {
                    planet: "The Hub".to_string()
                }
            ),
            3
        );
    }

    #[test]
    fn a_row_that_does_not_read_refuses_the_whole_paste_with_its_line() {
        let paste = format!(
            "{HEADER}Shrapnel\t40000\t4.0000\tCARRIED\t\nIron Stone\tmany\t1.30\tCARRIED\t\n"
        );
        assert_eq!(
            holdings_from_paste(&paste),
            Err(PasteRefusal::Row {
                line: 3,
                fault: PasteRowFault::Quantity
            })
        );
        let finer = format!("{HEADER}Iron Stone\t10\t1.305\tCARRIED\t\n");
        assert_eq!(
            holdings_from_paste(&finer),
            Err(PasteRefusal::Row {
                line: 2,
                fault: PasteRowFault::Value(ValueRefusal::NotWholePec)
            })
        );
    }

    #[test]
    fn the_changes_export_or_a_bare_header_is_not_an_inventory() {
        assert_eq!(
            holdings_from_paste("Name\tQuantity\tValue(PED)\nShrapnel\t-10\t-0.0010\n"),
            Err(PasteRefusal::NoHeader)
        );
        assert_eq!(holdings_from_paste(HEADER), Err(PasteRefusal::NoRows));
        assert_eq!(holdings_from_paste("  \n"), Err(PasteRefusal::NoHeader));
    }
}
