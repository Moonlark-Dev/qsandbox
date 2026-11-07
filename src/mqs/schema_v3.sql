-- MQS schema v3: per-bot interface whitelist (invite-only APIs)
ALTER TABLE bots ADD COLUMN whitelisted INTEGER NOT NULL DEFAULT 0;
