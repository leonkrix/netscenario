//! CRC-32 as used by Ethernet, zlib and PNG (the "IEEE" polynomial), written by hand because the agent has
//! no other use for a checksum library and a dependency that reads every packet is code to trust (D-040).
//!
//! The parameters: polynomial `0x04C11DB7` in its reflected form `0xEDB88320`, start value `0xFFFFFFFF`,
//! input and output reflected, final exclusive or with `0xFFFFFFFF`. The check value, the CRC-32 of the nine
//! bytes `123456789`, is `0xCBF43926`; a test pins it and compares the fast form below with a slow
//! bit-by-bit one on data of every length, so that a mistake in the tables cannot pass.
//!
//! The fast form is "slicing by 8": eight bytes per step, from eight tables of 256 entries that are computed
//! when the program is built. A packet of 1,472 bytes takes well under a microsecond.

/// The reflected polynomial.
const POLYNOMIAL: u32 = 0xEDB8_8320;

/// Table `k` has the CRC of a byte followed by `k` zero bytes. Table 0 is the usual byte-wise table.
const TABLES: [[u32; 256]; 8] = build_tables();

const fn build_tables() -> [[u32; 256]; 8] {
    let mut tables = [[0_u32; 256]; 8];
    let mut byte = 0_u32;
    while byte < 256 {
        let mut crc = byte;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ POLYNOMIAL
            } else {
                crc >> 1
            };
            bit += 1;
        }
        tables[0][byte as usize] = crc;
        byte += 1;
    }
    let mut byte = 0;
    while byte < 256 {
        let mut k = 1;
        while k < 8 {
            let before = tables[k - 1][byte];
            tables[k][byte] = (before >> 8) ^ tables[0][(before & 0xFF) as usize];
            k += 1;
        }
        byte += 1;
    }
    tables
}

/// A CRC-32 that is being computed over several pieces.
#[derive(Debug, Clone, Copy)]
pub struct Crc32(u32);

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32 {
    /// Starts a computation.
    #[must_use]
    pub const fn new() -> Self {
        Self(0xFFFF_FFFF)
    }

    /// Adds `bytes` to the computation.
    pub fn update(&mut self, bytes: &[u8]) {
        let mut crc = self.0;
        let (chunks, remainder) = bytes.as_chunks::<8>();
        for chunk in chunks {
            let low = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) ^ crc;
            let high = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
            crc = TABLES[7][(low & 0xFF) as usize]
                ^ TABLES[6][((low >> 8) & 0xFF) as usize]
                ^ TABLES[5][((low >> 16) & 0xFF) as usize]
                ^ TABLES[4][(low >> 24) as usize]
                ^ TABLES[3][(high & 0xFF) as usize]
                ^ TABLES[2][((high >> 8) & 0xFF) as usize]
                ^ TABLES[1][((high >> 16) & 0xFF) as usize]
                ^ TABLES[0][(high >> 24) as usize];
        }
        for &byte in remainder {
            crc = (crc >> 8) ^ TABLES[0][((crc ^ u32::from(byte)) & 0xFF) as usize];
        }
        self.0 = crc;
    }

    /// The CRC-32 of everything added so far. The computation can go on after this.
    #[must_use]
    pub const fn finish(self) -> u32 {
        self.0 ^ 0xFFFF_FFFF
    }
}

/// The CRC-32 of `bytes`.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(bytes);
    crc.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The definition, one bit at a time. Slow and obviously right: the reference for the fast form.
    fn bitwise(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFF_u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ POLYNOMIAL
                } else {
                    crc >> 1
                };
            }
        }
        crc ^ 0xFFFF_FFFF
    }

    /// The value that every CRC-32 table lists for the nine characters `123456789`.
    #[test]
    fn the_check_value_is_the_published_one() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
    }

    #[test]
    fn the_fast_form_equals_the_definition_for_every_length() {
        // A sequence that is not periodic in 8, so that a table that is wrong at one of the eight positions
        // shows up
        let data: Vec<u8> = (0_u32..2000)
            .map(|n| u8::try_from((n * 131 + n / 7 + 17) % 256).unwrap_or(0))
            .collect();
        for length in 0..=data.len() {
            assert_eq!(crc32(&data[..length]), bitwise(&data[..length]), "{length}");
        }
    }

    /// Canary: the comparison above can fail, and the check value tells a wrong polynomial from a right one.
    #[test]
    fn a_wrong_polynomial_or_a_flipped_bit_gives_another_value() {
        let reflected_wrong = {
            let mut crc = 0xFFFF_FFFF_u32;
            for &byte in b"123456789" {
                crc ^= u32::from(byte);
                for _ in 0..8 {
                    // The CRC-32C polynomial instead of the IEEE one
                    crc = if crc & 1 == 1 {
                        (crc >> 1) ^ 0x82F6_3B78
                    } else {
                        crc >> 1
                    };
                }
            }
            crc ^ 0xFFFF_FFFF
        };
        assert_ne!(reflected_wrong, 0xCBF4_3926);
        let mut data = vec![0x5A_u8; 100];
        let before = crc32(&data);
        for position in 0..data.len() {
            for bit in 0..8 {
                data[position] ^= 1 << bit;
                assert_ne!(crc32(&data), before, "byte {position} bit {bit}");
                data[position] ^= 1 << bit;
            }
        }
    }

    #[test]
    fn pieces_give_the_same_value_as_the_whole() {
        let data: Vec<u8> = (0..=255).cycle().take(1000).collect();
        for cut in [0, 1, 7, 8, 9, 500, 999, 1000] {
            let mut crc = Crc32::new();
            crc.update(&data[..cut]);
            crc.update(&data[cut..]);
            assert_eq!(crc.finish(), crc32(&data), "{cut}");
        }
    }
}
