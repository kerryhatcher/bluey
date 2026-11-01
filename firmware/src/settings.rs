//! Settings that survive a reset.
//!
//! They live in the first sector of the `nvs` data partition. Nothing else on the device uses
//! that partition (the WiFi driver is run without NVS), so it is used directly instead of
//! pulling in the NVS format for a single value.

use alloc::vec;
use bluey_core::proto::Mode;
use esp_bootloader_esp_idf::partitions::{
    DataPartitionSubType, Error, FlashRegion, PARTITION_TABLE_MAX_LEN, PartitionType, read_partition_table,
};
use esp_hal::peripherals::FLASH;
use esp_storage::FlashStorage;
use log::warn;

const SECTOR_LEN: u32 = FlashStorage::SECTOR_SIZE;

/// What is stored: a marker, then the mode and its complement, so a half-written or
/// unrelated sector is not mistaken for a setting.
const MAGIC: [u8; 4] = *b"bluy";
const RECORD_LEN: usize = 8;

pub struct Settings {
    flash: FlashStorage<'static>,
}

impl Settings {
    pub fn new(flash: FLASH<'static>) -> Self {
        Self { flash: FlashStorage::new(flash) }
    }

    /// The mode saved before the last reset, if there is one.
    pub fn load_mode(&mut self) -> Option<Mode> {
        let mut record = [0u8; RECORD_LEN];
        self.with_region(|region| region.read(0, &mut record))?;
        decode(&record)
    }

    /// Saves the mode, unless it is already what's stored.
    ///
    /// This erases a flash sector, which stalls the CPU for a few milliseconds, so don't call
    /// it while the radio is busy.
    pub fn save_mode(&mut self, mode: Mode) {
        if self.load_mode() == Some(mode) {
            return;
        }
        let record = encode(mode);
        self.with_region(|region| {
            region.erase(0, SECTOR_LEN)?;
            region.write(0, &record)
        });
    }

    /// Runs `f` on the `nvs` partition. Failures are logged, as there's nothing else to do
    /// about them: the device just won't remember its mode.
    fn with_region<T>(&mut self, f: impl FnOnce(&mut FlashRegion<'_, 'static>) -> Result<T, Error>) -> Option<T> {
        let mut table = vec![0u8; PARTITION_TABLE_MAX_LEN];
        let entry = read_partition_table(&mut self.flash, &mut table)
            .and_then(|table| table.find_partition(PartitionType::Data(DataPartitionSubType::Nvs)));
        let entry = match entry {
            Ok(Some(entry)) => entry,
            Ok(None) => {
                warn!("no nvs partition, can't save settings");
                return None;
            }
            Err(e) => {
                warn!("can't read the partition table: {e:?}");
                return None;
            }
        };
        match f(&mut entry.as_flash_region(&mut self.flash)) {
            Ok(value) => Some(value),
            Err(e) => {
                warn!("can't access settings: {e:?}");
                None
            }
        }
    }
}

fn encode(mode: Mode) -> [u8; RECORD_LEN] {
    let byte = match mode {
        Mode::Ble => 0,
        Mode::Wifi => 1,
        Mode::Ieee802154 => 2,
    };
    [MAGIC[0], MAGIC[1], MAGIC[2], MAGIC[3], byte, !byte, 0, 0]
}

fn decode(record: &[u8; RECORD_LEN]) -> Option<Mode> {
    if record[..4] != MAGIC || record[4] != !record[5] {
        return None;
    }
    match record[4] {
        0 => Some(Mode::Ble),
        1 => Some(Mode::Wifi),
        2 => Some(Mode::Ieee802154),
        _ => None,
    }
}
