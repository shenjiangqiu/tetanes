//! `BNROM` (Mapper 034)
//!
//! <https://www.nesdev.org/wiki/BNROM>

use crate::{
    cart::Cart,
    common::{Clock, Regional, Reset},
    mapper::{Mapped, MappedRead, MappedWrite, Mapper, MemMap},
    mem::MemBanks,
    ppu::Mirroring,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[must_use]
pub struct Bnrom {
    pub mirroring: Mirroring,
    pub prg_rom_banks: MemBanks,
}

impl Bnrom {
    const PRG_ROM_WINDOW: usize = 32 * 1024;
    const CHR_RAM_SIZE: usize = 8 * 1024;

    pub fn load(cart: &mut Cart) -> Mapper {
        if cart.chr_ram.is_empty() {
            cart.add_chr_ram(Self::CHR_RAM_SIZE);
        }
        let bnrom = Self {
            mirroring: cart.mirroring(),
            prg_rom_banks: MemBanks::new(0x8000, 0xFFFF, cart.prg_rom.len(), Self::PRG_ROM_WINDOW),
        };
        bnrom.into()
    }
}

impl Mapped for Bnrom {
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }

    fn set_mirroring(&mut self, _mirroring: Mirroring) {}
}

impl MemMap for Bnrom {
    // PPU $0000..=$1FFF 8K CHR-RAM Bank Fixed
    // CPU $8000..=$FFFF 32K switchable PRG-ROM bank

    fn map_peek(&self, addr: u16) -> MappedRead {
        match addr {
            0x0000..=0x1FFF => MappedRead::Chr(usize::from(addr) & (Self::CHR_RAM_SIZE - 1)),
            0x8000..=0xFFFF => MappedRead::PrgRom(self.prg_rom_banks.translate(addr)),
            _ => MappedRead::Bus,
        }
    }

    fn map_write(&mut self, addr: u16, val: u8) -> MappedWrite {
        match addr {
            0x0000..=0x1FFF => return MappedWrite::Chr(addr.into(), val),
            // Support up to 8MB PRG-ROM
            0x8000..=0xFFFF => self.prg_rom_banks.set(0, val.into()),
            _ => (),
        }
        MappedWrite::Bus
    }
}

impl Clock for Bnrom {}
impl Regional for Bnrom {}
impl Reset for Bnrom {}
