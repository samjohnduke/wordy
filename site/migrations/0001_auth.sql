-- better-auth core tables plus the passkey, device-authorization and bearer plugins,
-- and Wordy's own `device` table. Dates are ISO-8601 text, booleans are 0/1.

CREATE TABLE user (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  email TEXT NOT NULL UNIQUE,
  emailVerified INTEGER NOT NULL DEFAULT 0,
  image TEXT,
  createdAt TEXT NOT NULL,
  updatedAt TEXT NOT NULL
);

CREATE TABLE session (
  id TEXT PRIMARY KEY NOT NULL,
  expiresAt TEXT NOT NULL,
  token TEXT NOT NULL UNIQUE,
  createdAt TEXT NOT NULL,
  updatedAt TEXT NOT NULL,
  ipAddress TEXT,
  userAgent TEXT,
  userId TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE
);
CREATE INDEX session_userId_idx ON session(userId);

CREATE TABLE account (
  id TEXT PRIMARY KEY NOT NULL,
  accountId TEXT NOT NULL,
  providerId TEXT NOT NULL,
  userId TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
  accessToken TEXT,
  refreshToken TEXT,
  idToken TEXT,
  accessTokenExpiresAt TEXT,
  refreshTokenExpiresAt TEXT,
  scope TEXT,
  password TEXT,
  createdAt TEXT NOT NULL,
  updatedAt TEXT NOT NULL
);
CREATE INDEX account_userId_idx ON account(userId);

CREATE TABLE verification (
  id TEXT PRIMARY KEY NOT NULL,
  identifier TEXT NOT NULL,
  value TEXT NOT NULL,
  expiresAt TEXT NOT NULL,
  createdAt TEXT NOT NULL,
  updatedAt TEXT NOT NULL
);
CREATE INDEX verification_identifier_idx ON verification(identifier);

CREATE TABLE passkey (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT,
  publicKey TEXT NOT NULL,
  userId TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
  credentialID TEXT NOT NULL UNIQUE,
  counter INTEGER NOT NULL,
  deviceType TEXT NOT NULL,
  backedUp INTEGER NOT NULL,
  transports TEXT,
  createdAt TEXT,
  aaguid TEXT
);
CREATE INDEX passkey_userId_idx ON passkey(userId);

CREATE TABLE deviceCode (
  id TEXT PRIMARY KEY NOT NULL,
  deviceCode TEXT NOT NULL,
  userCode TEXT NOT NULL,
  userId TEXT,
  expiresAt TEXT NOT NULL,
  status TEXT NOT NULL,
  lastPolledAt TEXT,
  pollingInterval INTEGER,
  clientId TEXT,
  scope TEXT
);
CREATE INDEX deviceCode_deviceCode_idx ON deviceCode(deviceCode);
CREATE INDEX deviceCode_userCode_idx ON deviceCode(userCode);

-- A linked copy of the app. One row per bearer session created through the device flow.
CREATE TABLE device (
  id TEXT PRIMARY KEY NOT NULL,
  userId TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
  sessionId TEXT NOT NULL UNIQUE REFERENCES session(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  platform TEXT NOT NULL,
  appVersion TEXT,
  createdAt TEXT NOT NULL,
  lastSeenAt TEXT NOT NULL
);
CREATE INDEX device_userId_idx ON device(userId);
