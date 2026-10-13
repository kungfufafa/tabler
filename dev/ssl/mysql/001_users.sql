-- mysql-ssl users; the server runs with require_secure_transport=ON.
-- Any client certificate signed by dev/certs/ca.crt (through the intermediate), no password.
CREATE USER 'tabler_x509'@'%' REQUIRE X509;
-- Only the certificate whose subject is exactly this, no password.
CREATE USER 'tabler_subject'@'%' REQUIRE SUBJECT '/O=Tabler Dev/CN=tabler_cert';
-- Password over TLS, no client certificate.
CREATE USER 'tabler_pw'@'%' IDENTIFIED BY 'tabler' REQUIRE SSL;
GRANT ALL ON tabler_dev.* TO 'tabler_x509'@'%', 'tabler_subject'@'%', 'tabler_pw'@'%';
