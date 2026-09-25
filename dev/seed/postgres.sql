-- Sample "shop" schema for trying the explorer, grid and navigation.
create schema if not exists shop;

create table if not exists shop.customers (
  id bigserial primary key,
  email text not null unique,
  name text not null,
  country char(2) not null default 'CO',
  created_at timestamptz not null default now()
);
comment on table shop.customers is 'People who placed at least one order';

create table if not exists shop.products (
  sku text primary key,
  name text not null,
  price numeric(12, 2) not null check (price >= 0),
  tags text[] not null default '{}',
  attributes jsonb
);

create table if not exists shop.orders (
  id bigserial primary key,
  customer_id bigint not null references shop.customers (id),
  status text not null default 'pending',
  placed_at timestamptz not null default now()
);

create table if not exists shop.order_items (
  order_id bigint not null references shop.orders (id) on delete cascade,
  line smallint not null,
  sku text not null references shop.products (sku),
  quantity int not null,
  unit_price numeric(12, 2) not null,
  primary key (order_id, line)
);

create or replace view shop.order_totals as
  select o.id, o.customer_id, sum(i.quantity * i.unit_price) as total
  from shop.orders o join shop.order_items i on i.order_id = o.id
  group by o.id;

insert into shop.customers (email, name, country)
select 'user' || g || '@example.com', 'Customer ' || g, (array['CO', 'MX', 'US', 'ES'])[1 + g % 4]
from generate_series(1, 5000) g
on conflict do nothing;

insert into shop.products (sku, name, price, tags, attributes)
select 'SKU-' || lpad(g::text, 4, '0'), 'Product ' || g, round((random() * 200)::numeric, 2),
       array['tag' || g % 5], jsonb_build_object('color', (array['red', 'green', 'blue'])[1 + g % 3])
from generate_series(1, 200) g
on conflict do nothing;

insert into shop.orders (customer_id, status, placed_at)
select 1 + g % 5000, (array['pending', 'paid', 'shipped'])[1 + g % 3], now() - g * interval '7 minutes'
from generate_series(1, 50000) g
where not exists (select 1 from shop.orders);

insert into shop.order_items (order_id, line, sku, quantity, unit_price)
select o.id, l, 'SKU-' || lpad((1 + (o.id * l) % 200)::text, 4, '0'), 1 + (o.id + l) % 4, 9.99
from shop.orders o cross join generate_series(1, 3) l
on conflict do nothing;
