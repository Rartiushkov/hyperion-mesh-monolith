-- Hyperion Mesh MVP schema for Supabase / PostgreSQL
-- Run this in Supabase SQL Editor or via psql $DATABASE_URL

-- Users / card holders
CREATE TABLE IF NOT EXISTS users (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now(),
    kyc_status      VARCHAR(32) DEFAULT 'pending',  -- pending | approved | rejected
    tx_count        INT DEFAULT 0,
    kyc_level       TEXT DEFAULT 'BASIC',
    provider_user_id VARCHAR(128),
    phone           VARCHAR(64),
    email           VARCHAR(256),
    external_ref    VARCHAR(128) UNIQUE,             -- Wallester/Airwallex customer id
    stable_balance  BIGINT DEFAULT 0,                  -- micro-units of Hyperion Coin
    eurc_balance    BIGINT DEFAULT 0,                  -- micro-EURC
    metadata        JSONB DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_users_external_ref ON users(external_ref);
CREATE INDEX IF NOT EXISTS idx_users_kyc_status ON users(kyc_status);
CREATE INDEX IF NOT EXISTS idx_users_kyc_level ON users(kyc_level);

ALTER TABLE users ADD COLUMN IF NOT EXISTS tx_count INT DEFAULT 0;
ALTER TABLE users ADD COLUMN IF NOT EXISTS kyc_level TEXT DEFAULT 'BASIC';
ALTER TABLE users ADD COLUMN IF NOT EXISTS provider_user_id VARCHAR(128);

-- Payment / card authorization history (WAL)
CREATE TABLE IF NOT EXISTS payments (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at      TIMESTAMPTZ DEFAULT now(),
    user_id         UUID REFERENCES users(id) ON DELETE SET NULL,
    type            VARCHAR(32) NOT NULL,              -- deposit | card_auth | swap | withdraw
    status          VARCHAR(32) NOT NULL,              -- approved | rejected | pending
    amount_usdt     BIGINT,                            -- micro-USDT / Hyperion Coin
    amount_eurc     BIGINT,                            -- micro-EURC
    fx_rate         BIGINT,                            -- 1e6 fixed-point
    merchant_id     VARCHAR(128),
    merchant_country VARCHAR(4),
    raw_ingress_ns  BIGINT,                            -- measured latency in nanoseconds
    block_hash      BYTEA,                             -- optional PoPP chain block hash
    metadata        JSONB DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_payments_user_id ON payments(user_id);
CREATE INDEX IF NOT EXISTS idx_payments_created_at ON payments(created_at DESC);

-- Function to auto-update updated_at timestamp
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS update_users_updated_at ON users;
CREATE TRIGGER update_users_updated_at
BEFORE UPDATE ON users
FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
