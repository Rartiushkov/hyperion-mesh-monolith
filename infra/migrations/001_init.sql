-- Hyperion Mesh operational schema for Supabase / PostgreSQL
-- Data minimization: no passport, no full name, no card PAN, no scans.

create extension if not exists pgcrypto;

create table if not exists users (
    id uuid primary key default gen_random_uuid(),
    address_l2 text not null unique,
    tx_count int not null default 0,
    kyc_level text not null default 'BASIC'
);

create index if not exists idx_users_address_l2 on users(address_l2);
create index if not exists idx_users_kyc_level on users(kyc_level);

alter table users add column if not exists address_l2 text;
alter table users add column if not exists tx_count int default 0;
alter table users add column if not exists kyc_level text default 'BASIC';

create unique index if not exists idx_users_address_l2_unique on users(address_l2);

alter table users drop column if exists created_at;
alter table users drop column if exists updated_at;
alter table users drop column if exists kyc_status;
alter table users drop column if exists provider_user_id;
alter table users drop column if exists phone;
alter table users drop column if exists email;
alter table users drop column if exists external_ref;
alter table users drop column if exists stable_balance;
alter table users drop column if exists eurc_balance;
alter table users drop column if exists metadata;

create table if not exists transactions (
    id uuid primary key default gen_random_uuid(),
    created_at timestamptz not null default now(),
    address_l2 text not null references users(address_l2) on delete cascade,
    account_id text not null,
    transaction_kind text not null,
    status text not null,
    amount_minor bigint not null default 0,
    currency text not null,
    provider text not null,
    reference_id text not null default '',
    metadata jsonb not null default '{}'::jsonb
);

create index if not exists idx_transactions_address_l2 on transactions(address_l2);
create index if not exists idx_transactions_created_at on transactions(created_at desc);
create index if not exists idx_transactions_status on transactions(status);
