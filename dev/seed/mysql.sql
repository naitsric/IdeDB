-- Sample "shop" database for trying the explorer, grid and navigation.
create database if not exists shop;
use shop;

create table if not exists customers (
  id bigint unsigned auto_increment primary key,
  email varchar(255) not null unique,
  name varchar(255) not null,
  country char(2) not null default 'CO',
  created_at timestamp not null default current_timestamp
) comment 'People who placed at least one order';

create table if not exists products (
  sku varchar(16) primary key,
  name varchar(255) not null,
  price decimal(12, 2) not null,
  attributes json
);

create table if not exists orders (
  id bigint unsigned auto_increment primary key,
  customer_id bigint unsigned not null,
  status enum('pending', 'paid', 'shipped') not null default 'pending',
  placed_at datetime not null default current_timestamp,
  foreign key (customer_id) references customers (id)
);

create table if not exists order_items (
  order_id bigint unsigned not null,
  line smallint not null,
  sku varchar(16) not null,
  quantity int not null,
  unit_price decimal(12, 2) not null,
  primary key (order_id, line),
  foreign key (order_id) references orders (id) on delete cascade,
  foreign key (sku) references products (sku)
);

create or replace view order_totals as
  select o.id, o.customer_id, sum(i.quantity * i.unit_price) as total
  from orders o join order_items i on i.order_id = o.id
  group by o.id;

set session cte_max_recursion_depth = 100000;

insert ignore into customers (id, email, name, country)
with recursive g(n) as (select 1 union all select n + 1 from g where n < 5000)
select n, concat('user', n, '@example.com'), concat('Customer ', n), elt(1 + n % 4, 'CO', 'MX', 'US', 'ES') from g;

insert ignore into products (sku, name, price, attributes)
with recursive g(n) as (select 1 union all select n + 1 from g where n < 200)
select concat('SKU-', lpad(n, 4, '0')), concat('Product ', n), round(rand() * 200, 2),
       json_object('color', elt(1 + n % 3, 'red', 'green', 'blue')) from g;

insert ignore into orders (id, customer_id, status, placed_at)
with recursive g(n) as (select 1 union all select n + 1 from g where n < 50000)
select n, 1 + n % 5000, elt(1 + n % 3, 'pending', 'paid', 'shipped'), now() - interval (n * 7) minute from g;

insert ignore into order_items (order_id, line, sku, quantity, unit_price)
select o.id, l.n, concat('SKU-', lpad(1 + (o.id * l.n) % 200, 4, '0')), 1 + (o.id + l.n) % 4, 9.99
from orders o cross join (select 1 as n union all select 2 union all select 3) l;

-- The app user is created by the image before init scripts run.
grant all on shop.* to 'idedb'@'%';
