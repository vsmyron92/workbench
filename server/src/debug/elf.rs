//! What an executable is built for, read from its ELF header: a remote target's gdb has
//! to know its architecture (`arm-none-eabi-gdb` cannot debug RISC-V), and the program
//! says which one it is.

use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86,
    X86_64,
    Arm,
    Aarch64,
    Riscv,
    Xtensa,
    Avr,
    Msp430,
    Other,
}

impl Arch {
    pub fn from_machine(m: u16) -> Arch {
        match m {
            3 => Arch::X86,
            62 => Arch::X86_64,
            40 => Arch::Arm,
            183 => Arch::Aarch64,
            243 => Arch::Riscv,
            94 => Arch::Xtensa,
            83 => Arch::Avr,
            105 => Arch::Msp430,
            _ => Arch::Other,
        }
    }

    /// The architecture of the computer Workbench runs on.
    pub fn host() -> Arch {
        match std::env::consts::ARCH {
            "x86" => Arch::X86,
            "x86_64" => Arch::X86_64,
            "arm" => Arch::Arm,
            "aarch64" => Arch::Aarch64,
            "riscv32" | "riscv64" => Arch::Riscv,
            _ => Arch::Other,
        }
    }
}

/// The `e_machine` of the ELF file at `path`; none for anything else (a script, a
/// missing file, a PE executable).
pub fn machine(path: &Path) -> Option<u16> {
    let mut head = [0u8; 20];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    machine_of(&head)
}

fn machine_of(head: &[u8]) -> Option<u16> {
    if head.len() < 20 || &head[..4] != b"\x7fELF" {
        return None;
    }
    let bytes = [head[18], head[19]];
    match head[5] {
        1 => Some(u16::from_le_bytes(bytes)),
        2 => Some(u16::from_be_bytes(bytes)),
        _ => None,
    }
}

pub fn arch(path: &Path) -> Option<Arch> {
    machine(path).map(Arch::from_machine)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(data: u8, machine: [u8; 2]) -> Vec<u8> {
        let mut h = vec![0u8; 20];
        h[..4].copy_from_slice(b"\x7fELF");
        h[4] = 1;
        h[5] = data;
        h[18..20].copy_from_slice(&machine);
        h
    }

    #[test]
    fn the_machine_comes_from_the_header_in_either_byte_order() {
        assert_eq!(machine_of(&header(1, 40u16.to_le_bytes())), Some(40));
        assert_eq!(machine_of(&header(2, 40u16.to_be_bytes())), Some(40));
        assert_eq!(Arch::from_machine(40), Arch::Arm);
        assert_eq!(Arch::from_machine(243), Arch::Riscv);
        assert_eq!(Arch::from_machine(62), Arch::X86_64);
        assert_eq!(Arch::from_machine(9999), Arch::Other);
        // Not an ELF file, a cut one, an unknown byte order.
        assert_eq!(machine_of(b"#!/bin/sh\nexit 0\n......"), None);
        assert_eq!(machine_of(&header(1, [40, 0])[..10]), None);
        assert_eq!(machine_of(&header(0, [40, 0])), None);
    }

    #[test]
    fn files_are_read_without_being_trusted() {
        let d = tempfile::tempdir().unwrap();
        let elf = d.path().join("fw.elf");
        std::fs::write(&elf, header(1, 40u16.to_le_bytes())).unwrap();
        assert_eq!(arch(&elf), Some(Arch::Arm));
        assert_eq!(arch(&d.path().join("missing.elf")), None);
        std::fs::write(d.path().join("short"), b"\x7fELF").unwrap();
        assert_eq!(arch(&d.path().join("short")), None);
        assert_eq!(arch(d.path()), None, "a directory");
    }
}
