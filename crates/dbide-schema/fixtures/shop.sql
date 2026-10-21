CREATE SCHEMA shop;
CREATE TABLE shop.customer (
    id bigint CONSTRAINT customer_pk PRIMARY KEY,
    name text NOT NULL,
    email text
);
CREATE UNIQUE INDEX customer_email ON shop.customer (email);
CREATE TABLE shop.orders (
    id bigint CONSTRAINT orders_pk PRIMARY KEY,
    customer_id bigint NOT NULL,
    total numeric(18,2) NOT NULL DEFAULT 0,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP
);
ALTER TABLE shop.orders ADD CONSTRAINT orders_customer_fk FOREIGN KEY (customer_id)
    REFERENCES shop.customer(id) ON DELETE RESTRICT;
