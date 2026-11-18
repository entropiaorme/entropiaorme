-- How far the PED Card moved across the step a record was seen in, kept on
-- the record's own provenance row. Additive and forward-only.
--
-- A stock record or a listing outcome made from the account remembers the
-- step it was seen in: the two readings and when each was taken. The Card's
-- move across that step was looked up from the readings each time the row
-- was shown, which made the stock records the one place outside the account
-- side that read a reading. It is a fact about the step, known when the
-- record is made, so it is stored with the rest of the step, in PEC.
--
-- NULL where it was not known: a row whose readings are already gone, or a
-- record made without them. A stored change stays when its readings are
-- later removed.

ALTER TABLE stock_records_from_account ADD COLUMN card_change_pec INTEGER;
ALTER TABLE listing_outcomes_from_account ADD COLUMN card_change_pec INTEGER;

UPDATE stock_records_from_account
SET card_change_pec =
    (SELECT a.balance_pec FROM account_readings a WHERE a.id = reading_id)
  - (SELECT b.balance_pec FROM account_readings b WHERE b.id = prev_reading_id);

UPDATE listing_outcomes_from_account
SET card_change_pec =
    (SELECT a.balance_pec FROM account_readings a WHERE a.id = reading_id)
  - (SELECT b.balance_pec FROM account_readings b WHERE b.id = prev_reading_id);
