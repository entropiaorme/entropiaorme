"""Unit tests for the repair-cost OCR post-processing.

``RepairOcrService._parse_cost`` turns the recogniser's raw text into a PED
number: it normalises a comma decimal separator and stray spaces, then reads
the first numeric run. These cases pin every branch of that parse independent
of the OCR model, so the mutation campaign over ``repair_ocr.py`` has assertions
to fail against.
"""

import pytest

from backend.services.repair_ocr import RepairOcrService

parse = RepairOcrService._parse_cost


@pytest.mark.parametrize(
    "text,expected",
    [
        ("0.05", 0.05),  # the recorded repair capture's reading
        ("42", 42.0),  # bare integer, no decimal point
        ("5.", 5.0),  # trailing dot, no fractional digits
        ("1,50", 1.50),  # comma decimal separator is normalised to a dot
        ("12 . 34", 12.34),  # stray spaces around the dot are stripped
        ("1 234.56", 1234.56),  # a thousands space is stripped, not a separator
        ("PED 3.14", 3.14),  # leading label text is skipped to the number
        ("cost: 7.20 PED", 7.20),  # number embedded between words
    ],
)
def test_parse_cost_reads_the_number(text, expected):
    assert parse(text) == pytest.approx(expected)


@pytest.mark.parametrize("text", ["", "   ", "no digits here", "PED", "...", "$"])
def test_parse_cost_returns_zero_when_no_number(text):
    """A reading with no numeric run is a clean 0.0, not an error."""
    assert parse(text) == 0.0


def test_parse_cost_comma_changes_the_value():
    """The comma->dot normalisation is load-bearing: without it 1,50 reads as 1."""
    assert parse("1,50") == pytest.approx(1.50)
    assert parse("1,50") != pytest.approx(1.0)


def test_parse_cost_takes_the_first_run_only():
    """Two numbers in the text: the first numeric run wins."""
    assert parse("1.25 then 9.99") == pytest.approx(1.25)
