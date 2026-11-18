-- Keyless tracking: how a session read the player's equipment intent, and
-- where each paid healing activation's intent came from.
--
-- tracking_sessions.input_mode is 'keys' when the session could observe
-- hotbar presses (or chose not to, as every earlier session did), and
-- 'keyless' when it ran on a machine that cannot read keys, where healing
-- uses are inferred from the chat log. Every existing session is 'keys'.
--
-- healing_activations.intent_source names the evidence behind the paid use:
-- 'key' (a hotbar press), 'overlay' (the healer the player declared in the
-- overlay), or 'inferred' (keyless: a heal fitting the declared healer's
-- band and reload, with no press to confirm it). NULL for an activation
-- minted in review, which answers to the correction, not a live intent.
-- Existing live activations all came from key presses.
--
-- Additive and forward-only.
ALTER TABLE tracking_sessions ADD COLUMN input_mode TEXT NOT NULL DEFAULT 'keys'
    CHECK (input_mode IN ('keys', 'keyless'));
ALTER TABLE healing_activations ADD COLUMN intent_source TEXT
    CHECK (intent_source IS NULL OR intent_source IN ('key', 'overlay', 'inferred'));
UPDATE healing_activations SET intent_source = 'key'
    WHERE provenance IN ('direct', 'health_capped');
