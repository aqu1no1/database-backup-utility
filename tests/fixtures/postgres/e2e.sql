CREATE SCHEMA vendas;

CREATE TABLE public.clientes (
    id BIGSERIAL PRIMARY KEY,
    nome TEXT NOT NULL,
    email TEXT NOT NULL UNIQUE,
    ativo BOOLEAN NOT NULL DEFAULT TRUE,
    criado_em TIMESTAMPTZ NOT NULL,
    preferencias JSONB,
    tags TEXT[] NOT NULL DEFAULT '{}'
);

CREATE TABLE vendas.produtos (
    sku TEXT PRIMARY KEY,
    descricao TEXT NOT NULL,
    preco NUMERIC(12, 2) NOT NULL CHECK (preco >= 0),
    imagem BYTEA
);

CREATE TABLE vendas.pedidos (
    id BIGSERIAL PRIMARY KEY,
    cliente_id BIGINT NOT NULL REFERENCES public.clientes (id),
    sku TEXT NOT NULL REFERENCES vendas.produtos (sku),
    quantidade INTEGER NOT NULL CHECK (quantidade > 0),
    observacao TEXT,
    feito_em TIMESTAMPTZ NOT NULL
);

CREATE INDEX pedidos_cliente_idx ON vendas.pedidos (cliente_id, feito_em DESC);

CREATE SEQUENCE vendas.numero_nota START 1000;
SELECT setval('vendas.numero_nota', 4242);

CREATE VIEW vendas.total_por_cliente AS
SELECT p.cliente_id, sum(p.quantidade * pr.preco) AS total
FROM vendas.pedidos p
JOIN vendas.produtos pr ON pr.sku = p.sku
GROUP BY p.cliente_id;

CREATE FUNCTION vendas.preco_com_desconto(valor NUMERIC, percentual NUMERIC)
RETURNS NUMERIC
LANGUAGE SQL
IMMUTABLE
AS $$ SELECT round(valor * (1 - percentual / 100), 2) $$;

INSERT INTO public.clientes (nome, email, ativo, criado_em, preferencias, tags)
SELECT
    CASE WHEN i % 7 = 0 THEN 'Cliente ' || i || ' — São João 🌵' ELSE 'Cliente ' || i END,
    'cliente' || i || '@exemplo.test',
    i % 5 <> 0,
    TIMESTAMPTZ '2026-01-01 08:00:00-03' + (i || ' hours')::INTERVAL,
    CASE WHEN i % 3 = 0 THEN NULL ELSE jsonb_build_object('idioma', 'pt-BR', 'nivel', i % 4, 'flags', jsonb_build_array(i % 2 = 0, 'x')) END,
    ARRAY['grupo-' || (i % 4), 'origem-' || (i % 2)]
FROM generate_series(1, 300) AS i;

INSERT INTO vendas.produtos (sku, descricao, preco, imagem)
SELECT
    'SKU-' || lpad(i::TEXT, 4, '0'),
    'Produto ' || i || CASE WHEN i % 10 = 0 THEN ' (edição "especial")' ELSE '' END,
    round((i * 3.17)::NUMERIC, 2),
    CASE WHEN i % 4 = 0 THEN NULL ELSE decode(md5(i::TEXT), 'hex') END
FROM generate_series(1, 50) AS i;

INSERT INTO vendas.pedidos (cliente_id, sku, quantidade, observacao, feito_em)
SELECT
    1 + (i % 300),
    'SKU-' || lpad((1 + i % 50)::TEXT, 4, '0'),
    1 + (i % 9),
    CASE WHEN i % 11 = 0 THEN E'linha 1\nlinha 2\ttab' ELSE NULL END,
    TIMESTAMPTZ '2026-02-01 00:00:00+00' + (i || ' minutes')::INTERVAL
FROM generate_series(1, 800) AS i;
