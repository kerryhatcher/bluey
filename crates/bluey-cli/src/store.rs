//! The GeoPackage that `bluey log` records into.
//!
//! A [GeoPackage](https://www.geopackage.org/) is a SQLite file with a few standard tables
//! that describe its layers, so GIS software such as QGIS opens it directly and puts the
//! observations on a map. Everything else (Python, DuckDB, Grafana, `sqlite3`) reads it as
//! the SQLite it is.
//!
//! There is one layer, `observations`: a row for each device in each snapshot, with the time,
//! how strong the signal was, and where the GPS said the computer was. It is only ever added to.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use rusqlite::{Connection, params};

use crate::record::Record;

/// Stored in the file header so software knows it is a GeoPackage: "GPKG".
const APPLICATION_ID: i64 = 0x4750_4B47;
/// GeoPackage 1.3.0.
const USER_VERSION: i64 = 10300;
/// WGS 84 latitude and longitude, which is what GPS gives.
const SRS_ID: i64 = 4326;

const TABLE: &str = "observations";

/// The directory for data the program keeps: `$XDG_DATA_HOME/bluey`, which is
/// `~/.local/share/bluey` unless set otherwise.
pub fn data_dir() -> Result<PathBuf> {
    data_dir_from(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
        .context("can't tell where to keep data: neither XDG_DATA_HOME nor HOME is set")
}

fn data_dir_from(xdg_data_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    // The spec says a relative XDG_DATA_HOME is invalid and must be ignored.
    let base = xdg_data_home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("bluey"))
}

/// Where the GeoPackage goes unless told otherwise.
pub fn default_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("bluey.gpkg"))
}

pub struct GeoPackage {
    conn: Connection,
    /// The area covered so far: min x, min y, max x, max y.
    extent: Option<[f64; 4]>,
}

impl GeoPackage {
    /// Opens the file, making it (and its folder) if there isn't one. A file that exists but
    /// isn't a GeoPackage is refused rather than written into.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;

        let application_id: i64 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
        if application_id != APPLICATION_ID && tables > 0 {
            bail!("{} is a database, but not a GeoPackage, so it won't be written to", path.display());
        }

        conn.execute_batch(&format!("PRAGMA application_id = {APPLICATION_ID}; PRAGMA user_version = {USER_VERSION};"))?;
        conn.execute_batch(SCHEMA)?;
        // Files made before advert data was recorded lack the column.
        let has_adv: bool = conn.query_row(
            "SELECT count(*) FROM pragma_table_info('observations') WHERE name = 'adv'",
            [],
            |r| r.get::<_, i64>(0).map(|n| n > 0),
        )?;
        if !has_adv {
            conn.execute_batch("ALTER TABLE observations ADD COLUMN adv BLOB")?;
        }
        conn.execute(
            "INSERT OR IGNORE INTO gpkg_contents (table_name, data_type, identifier, description, srs_id) \
             VALUES (?1, 'features', ?1, 'Devices heard by bluey scanners, with signal strength, time and position', ?2)",
            params![TABLE, SRS_ID],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO gpkg_geometry_columns (table_name, column_name, geometry_type_name, srs_id, z, m) \
             VALUES (?1, 'geom', 'POINT', ?2, 0, 0)",
            params![TABLE, SRS_ID],
        )?;

        let extent = conn.query_row(
            "SELECT min_x, min_y, max_x, max_y FROM gpkg_contents WHERE table_name = ?1",
            [TABLE],
            |r| Ok([r.get::<_, Option<f64>>(0)?, r.get(1)?, r.get(2)?, r.get(3)?]),
        )?;
        let extent = match extent {
            [Some(a), Some(b), Some(c), Some(d)] => Some([a, b, c, d]),
            _ => None,
        };
        Ok(Self { conn, extent })
    }

    /// Adds one row for every device in the snapshot. Returns how many rows that was.
    pub fn record(&mut self, record: &Record) -> Result<usize> {
        let (latitude, longitude, altitude, hdop, satellites, geometry) = match &record.gps {
            Some(g) if g.fix && g.latitude.is_some() && g.longitude.is_some() => {
                let (lat, lon) = (g.latitude.unwrap_or_default(), g.longitude.unwrap_or_default());
                (Some(lat), Some(lon), g.altitude_m, g.hdop, g.satellites, Some(point(lon, lat)))
            }
            // No fix: the signal is still worth keeping, it just has nowhere to go on a map.
            Some(g) => (None, None, None, None, g.satellites, None),
            None => (None, None, None, None, None, None),
        };

        let tx = self.conn.transaction()?;
        let mut rows = 0;
        {
            let mut insert = tx.prepare_cached(INSERT)?;
            let mut add = |kind: &str,
                           device: &str,
                           label: Option<&str>,
                           category: &str,
                           detail: Option<&str>,
                           channel: Option<u8>,
                           (rssi, min, max, count): (i8, i8, i8, u32),
                           (first, last): (Timestamp, Timestamp),
                           adv: Option<&[u8]>|
             -> rusqlite::Result<()> {
                insert.execute(params![
                    geometry,
                    time(record.time),
                    record.device,
                    record.port,
                    kind,
                    device,
                    label,
                    category,
                    detail,
                    channel,
                    rssi,
                    min,
                    max,
                    count,
                    time(first),
                    time(last),
                    latitude,
                    longitude,
                    altitude,
                    hdop,
                    satellites,
                    adv,
                ])?;
                rows += 1;
                Ok(())
            };

            for d in &record.ble {
                add("ble", &d.addr, d.name.as_deref(), d.kind, Some(d.addr_type), None,
                    (d.rssi, d.rssi_min, d.rssi_max, d.count), (d.first_seen, d.last_seen),
                    Some(d.adv.as_slice()).filter(|a| !a.is_empty()))?;
            }
            for ap in &record.wifi {
                // A hidden network has no name, which is not the same as a name that is empty.
                let ssid = Some(ap.ssid.as_str()).filter(|s| !s.is_empty());
                add("wifi", &ap.bssid, ssid, &ap.auth, None, Some(ap.channel),
                    (ap.rssi, ap.rssi_min, ap.rssi_max, ap.count), (ap.first_seen, ap.last_seen), None)?;
            }
            for n in &record.ieee802154 {
                let device = match &n.pan_id {
                    Some(pan) => format!("{pan}:{}", n.addr),
                    None => n.addr.clone(),
                };
                add("802154", &device, n.network_name.as_deref(), n.protocol, Some(n.addr_type), Some(n.channel),
                    (n.rssi, n.rssi_min, n.rssi_max, n.count), (n.first_seen, n.last_seen), None)?;
            }
        }

        // Layers say what area they cover, so a map can zoom to it.
        if let (Some(lat), Some(lon)) = (latitude, longitude) {
            let [min_x, min_y, max_x, max_y] = self.extent.unwrap_or([lon, lat, lon, lat]);
            let extent = [min_x.min(lon), min_y.min(lat), max_x.max(lon), max_y.max(lat)];
            tx.execute(
                "UPDATE gpkg_contents SET min_x = ?1, min_y = ?2, max_x = ?3, max_y = ?4, last_change = ?5 WHERE table_name = ?6",
                params![extent[0], extent[1], extent[2], extent[3], time(record.time), TABLE],
            )?;
            self.extent = Some(extent);
        }
        tx.commit()?;
        Ok(rows)
    }
}

/// A time as GeoPackage wants it: UTC, to the millisecond, `2026-11-01T03:54:09.233Z`.
pub(crate) fn time(t: Timestamp) -> String {
    t.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// A point in the GeoPackage geometry encoding: a short header (`GP`, version, flags, the
/// coordinate system) followed by the point as standard WKB.
fn point(longitude: f64, latitude: f64) -> Vec<u8> {
    let mut blob = Vec::with_capacity(29);
    blob.extend_from_slice(b"GP");
    blob.push(0); // version
    blob.push(0b0000_0001); // little endian, no envelope, not empty
    blob.extend_from_slice(&(SRS_ID as i32).to_le_bytes());
    blob.push(1); // WKB: little endian
    blob.extend_from_slice(&1u32.to_le_bytes()); // WKB: Point
    blob.extend_from_slice(&longitude.to_le_bytes());
    blob.extend_from_slice(&latitude.to_le_bytes());
    blob
}

const INSERT: &str = "INSERT INTO observations (geom, time, scanner, port, kind, device, label, category, detail, channel, \
     rssi, rssi_min, rssi_max, \"count\", first_seen, last_seen, latitude, longitude, altitude_m, hdop, satellites, adv) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)";

/// The tables every GeoPackage has, and the `observations` layer.
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS gpkg_spatial_ref_sys (
  srs_name TEXT NOT NULL,
  srs_id INTEGER PRIMARY KEY,
  organization TEXT NOT NULL,
  organization_coordsys_id INTEGER NOT NULL,
  definition TEXT NOT NULL,
  description TEXT
);
INSERT OR IGNORE INTO gpkg_spatial_ref_sys VALUES
  ('Undefined cartesian SRS', -1, 'NONE', -1, 'undefined', 'undefined cartesian coordinate reference system'),
  ('Undefined geographic SRS', 0, 'NONE', 0, 'undefined', 'undefined geographic coordinate reference system'),
  ('WGS 84 geodetic', 4326, 'EPSG', 4326,
   'GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563,AUTHORITY["EPSG","7030"]],AUTHORITY["EPSG","6326"]],PRIMEM["Greenwich",0,AUTHORITY["EPSG","8901"]],UNIT["degree",0.0174532925199433,AUTHORITY["EPSG","9122"]],AUTHORITY["EPSG","4326"]]',
   'longitude/latitude coordinates in decimal degrees on the WGS 84 spheroid');

CREATE TABLE IF NOT EXISTS gpkg_contents (
  table_name TEXT NOT NULL PRIMARY KEY,
  data_type TEXT NOT NULL,
  identifier TEXT UNIQUE,
  description TEXT DEFAULT '',
  last_change DATETIME NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  min_x DOUBLE,
  min_y DOUBLE,
  max_x DOUBLE,
  max_y DOUBLE,
  srs_id INTEGER,
  CONSTRAINT fk_gc_r_srs_id FOREIGN KEY (srs_id) REFERENCES gpkg_spatial_ref_sys(srs_id)
);

CREATE TABLE IF NOT EXISTS gpkg_geometry_columns (
  table_name TEXT NOT NULL,
  column_name TEXT NOT NULL,
  geometry_type_name TEXT NOT NULL,
  srs_id INTEGER NOT NULL,
  z TINYINT NOT NULL,
  m TINYINT NOT NULL,
  CONSTRAINT pk_geom_cols PRIMARY KEY (table_name, column_name),
  CONSTRAINT uk_gc_table_name UNIQUE (table_name),
  CONSTRAINT fk_gc_tn FOREIGN KEY (table_name) REFERENCES gpkg_contents(table_name),
  CONSTRAINT fk_gc_srs FOREIGN KEY (srs_id) REFERENCES gpkg_spatial_ref_sys (srs_id)
);

-- One row per device per snapshot. The geometry is empty when the GPS had no fix.
CREATE TABLE IF NOT EXISTS observations (
  fid INTEGER PRIMARY KEY AUTOINCREMENT,
  geom BLOB,
  time DATETIME NOT NULL,         -- when the snapshot arrived, UTC
  scanner TEXT NOT NULL,          -- the scanner's chip MAC, which doesn't change with its port
  port TEXT NOT NULL,
  kind TEXT NOT NULL,             -- ble, wifi or 802154
  device TEXT NOT NULL,           -- BLE address, WiFi BSSID, or 802.15.4 PAN:address
  label TEXT,                     -- BLE name, WiFi SSID, or Thread network name
  category TEXT NOT NULL,         -- what kind of device, the WiFi security, or Zigbee/Thread
  detail TEXT,                    -- BLE address type, or 802.15.4 address type
  channel INTEGER,                -- WiFi and 802.15.4 only
  rssi INTEGER NOT NULL,          -- dBm, the last reading before the snapshot
  rssi_min INTEGER NOT NULL,      -- weakest and strongest since the scanner first heard it
  rssi_max INTEGER NOT NULL,
  "count" INTEGER NOT NULL,       -- advertisements (BLE), scans (WiFi) or frames (802.15.4) heard
  first_seen DATETIME NOT NULL,
  last_seen DATETIME NOT NULL,
  latitude REAL,
  longitude REAL,
  altitude_m REAL,
  hdop REAL,
  satellites INTEGER,
  adv BLOB                        -- BLE only: the latest advertisement's data, as on the air
);
CREATE INDEX IF NOT EXISTS observations_time ON observations (time);
CREATE INDEX IF NOT EXISTS observations_device ON observations (kind, device, time);
"#;

#[cfg(test)]
mod tests {
    use bluey_core::adv::{AddrClass, Kind};
    use bluey_core::proto::{AccessPoint, BleDevice, Ieee802154Node, Mode, NodeAddr, PROTOCOL_VERSION, Protocol, Snapshot};

    use super::*;
    use crate::gps::Reading;

    fn snapshot() -> Snapshot {
        Snapshot {
            protocol: PROTOCOL_VERSION,
            device: [0x90, 0x64, 0x9B, 0xAA, 0x29, 0x00],
            mode: Mode::Ble,
            supported: vec![Mode::Ble],
            uptime_ms: 100_000,
            ble: vec![BleDevice {
                addr: [1, 2, 3, 4, 5, 0xAB],
                class: AddrClass::ResolvablePrivate,
                kind: Kind::AppleFindMy,
                name: Some("tag".to_string()),
                rssi: -61,
                rssi_min: -70,
                rssi_max: -55,
                count: 12,
                first_seen_ms: 10_000,
                last_seen_ms: 97_000,
                adv: vec![2, 0x01, 0x06, 4, 0xFF, 0x4C, 0x00, 0x12],
            }],
            wifi: vec![AccessPoint {
                bssid: [0xF4, 0x92, 0xBF, 0x92, 0x56, 0xC9],
                ssid: String::new(),
                channel: 6,
                auth: "Wpa2Personal".to_string(),
                rssi: -50,
                rssi_min: -52,
                rssi_max: -48,
                count: 3,
                first_seen_ms: 1,
                last_seen_ms: 100_000,
            }],
            ieee802154: vec![Ieee802154Node {
                pan_id: Some(0x9297),
                addr: NodeAddr::Short(0x3148),
                protocol: Protocol::Zigbee,
                network_name: None,
                channel: 25,
                rssi: -76,
                rssi_min: -78,
                rssi_max: -73,
                count: 17,
                first_seen_ms: 1,
                last_seen_ms: 99_000,
            }],
        }
    }

    fn reading(fix: bool, latitude: f64, longitude: f64) -> Reading {
        Reading {
            fix,
            satellites: Some(6),
            latitude: fix.then_some(latitude),
            longitude: fix.then_some(longitude),
            altitude_m: fix.then_some(140.0),
            hdop: fix.then_some(1.4),
            speed_mps: None,
            course_deg: None,
            age_ms: Some(10),
        }
    }

    /// A path in a fresh temporary folder, removed when dropped.
    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("bluey-test-{}-{}", std::process::id(), Timestamp::now().as_nanosecond()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join("nested").join("test.gpkg")
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn has_the_structure_of_a_geopackage() {
        let temp = Temp::new();
        let path = temp.file();
        drop(GeoPackage::open(&path).unwrap());

        let conn = Connection::open(&path).unwrap();
        assert_eq!(conn.query_row("PRAGMA application_id", [], |r| r.get::<_, i64>(0)).unwrap(), 0x4750_4B47);
        assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 10300);
        let layer: (String, String, i64) = conn
            .query_row("SELECT column_name, geometry_type_name, srs_id FROM gpkg_geometry_columns WHERE table_name = 'observations'", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(layer, ("geom".to_string(), "POINT".to_string(), 4326));
        let srs: i64 = conn.query_row("SELECT count(*) FROM gpkg_spatial_ref_sys WHERE srs_id IN (-1, 0, 4326)", [], |r| r.get(0)).unwrap();
        assert_eq!(srs, 3);
    }

    #[test]
    fn records_a_row_per_device_with_a_position() {
        let temp = Temp::new();
        let mut store = GeoPackage::open(&temp.file()).unwrap();
        let record = Record::new("/dev/ttyACM0", &snapshot(), Some(reading(true, 32.5, -83.25))).unwrap();
        assert_eq!(store.record(&record).unwrap(), 3);

        let conn = Connection::open(temp.file()).unwrap();
        let (kind, device, label, rssi, geom): (String, String, Option<String>, i64, Vec<u8>) = conn
            .query_row("SELECT kind, device, label, rssi, geom FROM observations WHERE kind = 'ble'", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap();
        assert_eq!((kind.as_str(), device.as_str(), label.as_deref(), rssi), ("ble", "AB:05:04:03:02:01", Some("tag"), -61));

        // The geometry is the GeoPackage header followed by a WKB point, longitude first.
        assert_eq!(&geom[..2], b"GP");
        assert_eq!(i32::from_le_bytes(geom[4..8].try_into().unwrap()), 4326);
        assert_eq!(u32::from_le_bytes(geom[9..13].try_into().unwrap()), 1);
        assert_eq!(f64::from_le_bytes(geom[13..21].try_into().unwrap()), -83.25);
        assert_eq!(f64::from_le_bytes(geom[21..29].try_into().unwrap()), 32.5);

        // A hidden network has no name; the others are keyed so devices on different networks can't collide.
        let hidden: Option<String> = conn.query_row("SELECT label FROM observations WHERE kind = 'wifi'", [], |r| r.get(0)).unwrap();
        assert_eq!(hidden, None);
        let node: String = conn.query_row("SELECT device FROM observations WHERE kind = '802154'", [], |r| r.get(0)).unwrap();
        assert_eq!(node, "9297:3148");

        // Times are in the form GeoPackage wants.
        let first: String = conn.query_row("SELECT first_seen FROM observations WHERE kind = 'ble'", [], |r| r.get(0)).unwrap();
        assert_eq!(first.len(), 24, "{first}");
        assert!(first.ends_with('Z') && first.as_bytes()[10] == b'T', "{first}");
    }

    #[test]
    fn keeps_ble_advert_data() {
        let temp = Temp::new();
        let mut store = GeoPackage::open(&temp.file()).unwrap();
        store.record(&Record::new("p", &snapshot(), None).unwrap()).unwrap();

        let conn = Connection::open(temp.file()).unwrap();
        let adv: Vec<u8> = conn.query_row("SELECT adv FROM observations WHERE kind = 'ble'", [], |r| r.get(0)).unwrap();
        assert_eq!(adv, [2, 0x01, 0x06, 4, 0xFF, 0x4C, 0x00, 0x12]);
        let others: i64 = conn.query_row("SELECT count(adv) FROM observations WHERE kind <> 'ble'", [], |r| r.get(0)).unwrap();
        assert_eq!(others, 0);
    }

    #[test]
    fn adds_the_adv_column_to_an_older_file() {
        let temp = Temp::new();
        drop(GeoPackage::open(&temp.file()).unwrap());
        // Make it look like a file from before advert data was recorded.
        Connection::open(temp.file()).unwrap().execute_batch("ALTER TABLE observations DROP COLUMN adv").unwrap();

        let mut store = GeoPackage::open(&temp.file()).unwrap();
        assert_eq!(store.record(&Record::new("p", &snapshot(), None).unwrap()).unwrap(), 3);
        let conn = Connection::open(temp.file()).unwrap();
        assert_eq!(conn.query_row("SELECT count(adv) FROM observations", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    }

    #[test]
    fn no_fix_keeps_the_signal_without_a_position() {
        let temp = Temp::new();
        let mut store = GeoPackage::open(&temp.file()).unwrap();
        store.record(&Record::new("p", &snapshot(), Some(reading(false, 0.0, 0.0))).unwrap()).unwrap();
        store.record(&Record::new("p", &snapshot(), None).unwrap()).unwrap();

        let conn = Connection::open(temp.file()).unwrap();
        let with_geometry: i64 = conn.query_row("SELECT count(geom) FROM observations", [], |r| r.get(0)).unwrap();
        let total: i64 = conn.query_row("SELECT count(*) FROM observations", [], |r| r.get(0)).unwrap();
        assert_eq!((with_geometry, total), (0, 6));
        // The extent stays unset until there is a point to put in it.
        let min_x: Option<f64> = conn.query_row("SELECT min_x FROM gpkg_contents", [], |r| r.get(0)).unwrap();
        assert_eq!(min_x, None);
    }

    #[test]
    fn reopening_adds_to_the_file_and_widens_the_extent() {
        let temp = Temp::new();
        {
            let mut store = GeoPackage::open(&temp.file()).unwrap();
            store.record(&Record::new("p", &snapshot(), Some(reading(true, 32.0, -83.0))).unwrap()).unwrap();
        }
        {
            let mut store = GeoPackage::open(&temp.file()).unwrap();
            store.record(&Record::new("p", &snapshot(), Some(reading(true, 33.0, -84.0))).unwrap()).unwrap();
        }
        let conn = Connection::open(temp.file()).unwrap();
        assert_eq!(conn.query_row("SELECT count(*) FROM observations", [], |r| r.get::<_, i64>(0)).unwrap(), 6);
        let extent: [f64; 4] = conn
            .query_row("SELECT min_x, min_y, max_x, max_y FROM gpkg_contents WHERE table_name = 'observations'", [], |r| {
                Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?])
            })
            .unwrap();
        assert_eq!(extent, [-84.0, 32.0, -83.0, 33.0]);
    }

    #[test]
    fn refuses_a_database_that_is_not_a_geopackage() {
        let temp = Temp::new();
        let path = temp.0.join("other.db");
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE notes (text TEXT);").unwrap();
        let error = GeoPackage::open(&path).err().expect("should be refused").to_string();
        assert!(error.contains("not a GeoPackage"), "{error}");
        // And it was left alone.
        let tables: i64 = Connection::open(&path).unwrap().query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0)).unwrap();
        assert_eq!(tables, 1);
    }

    #[test]
    fn data_goes_in_the_xdg_data_home() {
        let os = |s: &str| Some(OsString::from(s));
        assert_eq!(data_dir_from(os("/data"), os("/home/me")), Some(PathBuf::from("/data/bluey")));
        // Unset, empty or relative falls back to ~/.local/share, as the XDG spec says.
        assert_eq!(data_dir_from(None, os("/home/me")), Some(PathBuf::from("/home/me/.local/share/bluey")));
        assert_eq!(data_dir_from(os(""), os("/home/me")), Some(PathBuf::from("/home/me/.local/share/bluey")));
        assert_eq!(data_dir_from(os("relative/dir"), os("/home/me")), Some(PathBuf::from("/home/me/.local/share/bluey")));
        assert_eq!(data_dir_from(None, None), None);
    }
}
