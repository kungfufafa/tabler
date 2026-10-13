-- pg-ssl roles (see pg_hba.conf next to this file).
CREATE ROLE tabler_cert LOGIN;
CREATE ROLE tabler_ca LOGIN PASSWORD 'tabler';
CREATE ROLE tabler_full LOGIN PASSWORD 'tabler';
GRANT ALL ON DATABASE tabler_dev TO tabler_cert, tabler_ca, tabler_full;
GRANT ALL ON SCHEMA public TO tabler_cert, tabler_ca, tabler_full;
