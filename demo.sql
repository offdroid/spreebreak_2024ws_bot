CREATE TABLE IF NOT EXISTS users (
  id SERIAL PRIMARY KEY,
  username TEXT,
  first_name TEXT,
  last_name TEXT,
  team TEXT,
  created_at INT
);

CREATE TABLE IF NOT EXISTS forums (
  id SERIAL PRIMARY KEY,
  name TEXT,
  created_at INT
);

CREATE TABLE IF NOT EXISTS submissions (
  message_id SERIAL PRIMARY KEY,
  user int,
  team TEXT,
  date INT,
  caption TEXT,
  type INT
);

CREATE TABLE IF NOT EXISTS challenges (
  name TEXT PRIMARY KEY,
  short_name TEXT,
  desc TEXT,
  points INT
);
INSERT OR IGNORE INTO challenges
  (name, short_name, desc, points)
  VALUES ('döner_macht_schöner1', 'döner macht schöner1', 'Iss einen Döner', 1)
;
INSERT OR IGNORE INTO challenges
  (name, short_name, desc, points)
  VALUES ('döner_macht_schöner2', 'döner macht schöner2', 'Foto mit dem Dönermann', 1)
;

CREATE TABLE IF NOT EXISTS judgement (
  submission_id INT PRIMARY KEY,
  challenge_name TEXT,
  points INT,
  valid BOOLEAN
);

CREATE TABLE IF NOT EXISTS config (
  name TEXT PRIMARY KEY,
  value TEXT
);

CREATE TABLE IF NOT EXISTS safety_team (
  name TEXT PRIMARY KEY,
  phone TEXT,
  date TEXT
);
INSERT OR IGNORE INTO safety_team
  (name, phone, date)
  VALUES ('Max Mustermann', '+49 123', '2024-11-14')
;
