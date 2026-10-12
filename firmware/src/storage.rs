//! FAT 上の設定。主ファイルの更新前に、検証済みの旧設定をバックアップする。
use crate::settings::{ConfigError, MAX_CONFIG_BYTES, Settings};
use embedded_sdmmc::{
    BlockDevice, Directory, Mode, TimeSource, Timestamp, VolumeIdx, VolumeManager,
};

pub const CONFIG_FILE: &str = "STKCN.CFG";
pub const BACKUP_FILE: &str = "STKCN.BAK";

pub struct FileTime;

impl TimeSource for FileTime {
    fn get_timestamp(&self) -> Timestamp {
        // RTC が未設定でも FAT の有効な日付を使う。保存順の判定には日時を使わない。
        Timestamp {
            year_since_1970: 30,
            zero_indexed_month: 0,
            zero_indexed_day: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}

#[derive(Debug)]
pub enum Error<E: core::error::Error> {
    Filesystem(embedded_sdmmc::Error<E>),
    Config(ConfigError),
}

impl<E: core::error::Error> From<embedded_sdmmc::Error<E>> for Error<E> {
    fn from(value: embedded_sdmmc::Error<E>) -> Self {
        Self::Filesystem(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub settings: Settings,
    pub recovered: bool,
}

type Root<'a, D> = Directory<'a, D, FileTime, 4, 4, 1>;

fn read_file<D: BlockDevice>(
    root: &Root<'_, D>,
    name: &str,
) -> Result<Option<Settings>, Error<D::Error>> {
    let file = match root.open_file_in_dir(name, Mode::ReadOnly) {
        Ok(file) => file,
        Err(embedded_sdmmc::Error::NotFound) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if file.length() as usize > MAX_CONFIG_BYTES {
        return Err(Error::Config(ConfigError::TooLong));
    }
    let mut bytes = [0; MAX_CONFIG_BYTES];
    let mut count = 0;
    while !file.is_eof() && count < bytes.len() {
        let read = file.read(&mut bytes[count..])?;
        if read == 0 {
            return Err(Error::Config(ConfigError::Invalid));
        }
        count += read;
    }
    file.close()?;
    let text =
        core::str::from_utf8(&bytes[..count]).map_err(|_| Error::Config(ConfigError::Invalid))?;
    Settings::parse(text).map(Some).map_err(Error::Config)
}

fn write_file<D: BlockDevice>(
    root: &Root<'_, D>,
    name: &str,
    settings: Settings,
) -> Result<(), Error<D::Error>> {
    let file = root.open_file_in_dir(name, Mode::ReadWriteCreateOrTruncate)?;
    file.write(settings.encode().as_bytes())?;
    // close の失敗も保存失敗として扱い、UI に成功を返さない。
    file.close()?;
    if read_file(root, name)? != Some(settings) {
        return Err(Error::Config(ConfigError::Invalid));
    }
    Ok(())
}

pub fn load<D: BlockDevice>(device: D) -> Result<Option<Loaded>, Error<D::Error>> {
    let manager = VolumeManager::new(device, FileTime);
    let volume = manager.open_volume(VolumeIdx(0))?;
    let root = volume.open_root_dir()?;
    let primary = read_file(&root, CONFIG_FILE);
    if let Ok(Some(settings)) = primary {
        return Ok(Some(Loaded {
            settings,
            recovered: false,
        }));
    }
    if let Some(settings) = read_file(&root, BACKUP_FILE)? {
        return Ok(Some(Loaded {
            settings,
            recovered: true,
        }));
    }
    primary.map(|_| None)
}

pub fn save<D: BlockDevice>(device: D, settings: Settings) -> Result<(), Error<D::Error>> {
    let manager = VolumeManager::new(device, FileTime);
    let volume = manager.open_volume(VolumeIdx(0))?;
    let root = volume.open_root_dir()?;
    let previous = match read_file(&root, CONFIG_FILE) {
        Ok(Some(previous)) => {
            write_file(&root, BACKUP_FILE, previous)?;
            Some(previous)
        }
        Ok(None) | Err(Error::Config(_)) => None,
        Err(error) => return Err(error),
    };
    drop(root);
    drop(volume);
    // キャッシュ上の一致だけで成功扱いしないよう、検証は新しい manager で行う。
    let manager = VolumeManager::new(manager.free().0, FileTime);
    let volume = manager.open_volume(VolumeIdx(0))?;
    let root = volume.open_root_dir()?;
    if let Some(previous) = previous
        && read_file(&root, BACKUP_FILE)? != Some(previous)
    {
        return Err(Error::Config(ConfigError::Invalid));
    }
    write_file(&root, CONFIG_FILE, settings)?;
    drop(root);
    drop(volume);
    if load(manager.free().0)?
        != Some(Loaded {
            settings,
            recovered: false,
        })
    {
        return Err(Error::Config(ConfigError::Invalid));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::{Cell, RefCell};
    use embedded_sdmmc::{Block, BlockCount, BlockIdx};

    struct Disk {
        blocks: RefCell<std::vec::Vec<Block>>,
        fail_write: Cell<bool>,
    }

    #[derive(Debug)]
    struct DiskError;
    impl core::fmt::Display for DiskError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str("test disk I/O")
        }
    }
    impl core::error::Error for DiskError {}

    impl BlockDevice for &Disk {
        type Error = DiskError;
        fn read(&self, target: &mut [Block], start: BlockIdx) -> Result<(), DiskError> {
            let blocks = self.blocks.borrow();
            let source = blocks
                .get(start.0 as usize..start.0 as usize + target.len())
                .ok_or(DiskError)?;
            for (out, input) in target.iter_mut().zip(source) {
                out.contents.copy_from_slice(&input.contents);
            }
            Ok(())
        }
        fn write(&self, source: &[Block], start: BlockIdx) -> Result<(), DiskError> {
            if self.fail_write.get() {
                return Err(DiskError);
            }
            let mut blocks = self.blocks.borrow_mut();
            let target = blocks
                .get_mut(start.0 as usize..start.0 as usize + source.len())
                .ok_or(DiskError)?;
            for (out, input) in target.iter_mut().zip(source) {
                out.contents.copy_from_slice(&input.contents);
            }
            Ok(())
        }
        fn num_blocks(&self) -> Result<BlockCount, DiskError> {
            Ok(BlockCount(self.blocks.borrow().len() as u32))
        }
    }

    fn disk() -> Disk {
        // テスト専用の 4 MiB FAT16。実機コードにはフォーマット機能を持たせない。
        let mut blocks: std::vec::Vec<Block> = (0..8192).map(|_| Block::new()).collect();
        let mbr = &mut blocks[0].contents;
        mbr[450] = 0x06;
        mbr[454..458].copy_from_slice(&1u32.to_le_bytes());
        mbr[458..462].copy_from_slice(&8191u32.to_le_bytes());
        mbr[510..512].copy_from_slice(&[0x55, 0xaa]);
        let boot = &mut blocks[1].contents;
        boot[0..3].copy_from_slice(&[0xeb, 0x3c, 0x90]);
        boot[11..13].copy_from_slice(&512u16.to_le_bytes());
        boot[13] = 1;
        boot[14..16].copy_from_slice(&1u16.to_le_bytes());
        boot[16] = 2;
        boot[17..19].copy_from_slice(&512u16.to_le_bytes());
        boot[19..21].copy_from_slice(&8191u16.to_le_bytes());
        boot[21] = 0xf8;
        boot[22..24].copy_from_slice(&32u16.to_le_bytes());
        boot[38] = 0x29;
        boot[54..62].copy_from_slice(b"FAT16   ");
        boot[510..512].copy_from_slice(&[0x55, 0xaa]);
        for sector in [2, 34] {
            blocks[sector].contents[..4].copy_from_slice(&[0xf8, 0xff, 0xff, 0xff]);
        }
        Disk {
            blocks: RefCell::new(blocks),
            fail_write: Cell::new(false),
        }
    }

    #[test]
    fn fat_save_load_and_invalid_primary_recovery() {
        let disk = disk();
        {
            let manager = VolumeManager::new(&disk, FileTime);
            let volume = manager.open_volume(VolumeIdx(0)).unwrap();
            let root = volume.open_root_dir().unwrap();
            let file = root
                .open_file_in_dir("NOTES.TXT", Mode::ReadWriteCreateOrTruncate)
                .unwrap();
            file.write(b"preserve").unwrap();
            file.close().unwrap();
        }
        assert_eq!(load(&disk).unwrap(), None);
        let first = Settings::default();
        save(&disk, first).unwrap();
        assert_eq!(
            load(&disk).unwrap(),
            Some(Loaded {
                settings: first,
                recovered: false
            })
        );
        let second = Settings {
            motion: false,
            speech: false,
            ..first
        };
        save(&disk, second).unwrap();
        assert_eq!(load(&disk).unwrap().unwrap().settings, second);
        {
            let manager = VolumeManager::new(&disk, FileTime);
            let volume = manager.open_volume(VolumeIdx(0)).unwrap();
            let root = volume.open_root_dir().unwrap();
            let other = root.open_file_in_dir("NOTES.TXT", Mode::ReadOnly).unwrap();
            let mut content = [0; 8];
            assert_eq!(other.read(&mut content).unwrap(), 8);
            assert_eq!(&content, b"preserve");
            other.close().unwrap();
            let file = root
                .open_file_in_dir(CONFIG_FILE, Mode::ReadWriteCreateOrTruncate)
                .unwrap();
            file.write(b"version=1\n").unwrap();
            file.close().unwrap();
        }
        assert_eq!(
            load(&disk).unwrap(),
            Some(Loaded {
                settings: first,
                recovered: true
            })
        );
    }

    #[test]
    fn failed_backup_does_not_replace_primary() {
        let disk = disk();
        let original = Settings::default();
        save(&disk, original).unwrap();
        disk.fail_write.set(true);
        assert!(
            save(
                &disk,
                Settings {
                    enabled: false,
                    ..original
                }
            )
            .is_err()
        );
        disk.fail_write.set(false);
        assert_eq!(load(&disk).unwrap().unwrap().settings, original);
    }

    #[test]
    fn fat32_settings_round_trip() {
        // FAT32 として識別される最小クラスタ数を超えるテストイメージ。
        let mut blocks: std::vec::Vec<Block> = (0..70_000).map(|_| Block::new()).collect();
        let mbr = &mut blocks[0].contents;
        mbr[450] = 0x0c;
        mbr[454..458].copy_from_slice(&1u32.to_le_bytes());
        mbr[458..462].copy_from_slice(&69_999u32.to_le_bytes());
        mbr[510..512].copy_from_slice(&[0x55, 0xaa]);
        let boot = &mut blocks[1].contents;
        boot[..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
        boot[11..13].copy_from_slice(&512u16.to_le_bytes());
        boot[13] = 1;
        boot[14..16].copy_from_slice(&32u16.to_le_bytes());
        boot[16] = 2;
        boot[21] = 0xf8;
        boot[32..36].copy_from_slice(&69_999u32.to_le_bytes());
        boot[36..40].copy_from_slice(&550u32.to_le_bytes());
        boot[44..48].copy_from_slice(&2u32.to_le_bytes());
        boot[48..50].copy_from_slice(&1u16.to_le_bytes());
        boot[66] = 0x29;
        boot[82..90].copy_from_slice(b"FAT32   ");
        boot[510..512].copy_from_slice(&[0x55, 0xaa]);
        let info = &mut blocks[2].contents;
        info[..4].copy_from_slice(&0x41615252u32.to_le_bytes());
        info[484..488].copy_from_slice(&0x61417272u32.to_le_bytes());
        info[488..496].fill(0xff);
        info[508..512].copy_from_slice(&0xaa550000u32.to_le_bytes());
        for sector in [33, 583] {
            blocks[sector].contents[..12].copy_from_slice(&[
                0xf8, 0xff, 0xff, 0x0f, 0xff, 0xff, 0xff, 0x0f, 0xff, 0xff, 0xff, 0x0f,
            ]);
        }
        let disk = Disk {
            blocks: RefCell::new(blocks),
            fail_write: Cell::new(false),
        };
        assert_eq!(load(&disk).unwrap(), None);
        let settings = Settings {
            enabled: false,
            ..Settings::default()
        };
        save(&disk, settings).unwrap();
        assert_eq!(load(&disk).unwrap().unwrap().settings, settings);
    }
}
