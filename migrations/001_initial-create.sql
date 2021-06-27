CREATE TABLE "user" (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL CONSTRAINT user_name_unique UNIQUE,
    email TEXT NULL,
    pw_hash TEXT NOT NULL,
    is_admin BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE TABLE session (
    session_id UUID UNIQUE,
    user_id UUID NOT NULL REFERENCES "user"(id),
    PRIMARY KEY(session_id, user_id)
);
CREATE TABLE article (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL CONSTRAINT article_name_unique UNIQUE,
    created TIMESTAMP NOT NULL DEFAULT now(),
    creator_id UUID NOT NULL REFERENCES "user"(id)
);
CREATE TABLE revision (
    article_id UUID NOT NULL REFERENCES article(id),
    num BIGINT NOT NULL,
    content TEXT NOT NULL,
    author_id UUID NOT NULL REFERENCES "user"(id),
    created TIMESTAMP NOT NULL DEFAULT now(),
    PRIMARY KEY(article_id, num)
);
CREATE TABLE flags (
    name TEXT PRIMARY KEY,
    value BOOLEAN NOT NULL
);
