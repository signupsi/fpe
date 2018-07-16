use aes::{block_cipher_trait::generic_array::GenericArray, BlockCipher};
use byteorder::{BigEndian, WriteBytesExt};
use num_bigint::{BigInt, BigUint, Sign};
use num_traits::{
    identities::{One, Zero}, ToPrimitive,
};

pub trait NumeralString<R: RadixOps>: Sized {
    fn is_valid(&self, radix: &R) -> bool;

    fn len(&self) -> usize;
    fn split(&self, u: usize) -> (Self, Self);
    fn concat(a: Self, b: Self) -> Self;

    fn num_radix(&self, radix: &BigUint) -> BigUint;
    fn str_radix(x: BigUint, radix: &BigUint, m: usize) -> Self;
}

pub trait RadixOps {
    fn check_in_range(&self, n: u32) -> bool;
    /// Calculates b = ceil(ceil(v * log2(radix)) / 8).
    fn calculate_b(&self, v: usize) -> usize;
    fn to_biguint(&self) -> BigUint;
    fn to_u32(&self) -> u32;
}

/// A numeral string that supports radixes in [2..2^16).
pub struct FlexibleNumeralString(Vec<u16>);

impl From<Vec<u16>> for FlexibleNumeralString {
    fn from(v: Vec<u16>) -> Self {
        FlexibleNumeralString(v)
    }
}

impl From<FlexibleNumeralString> for Vec<u16> {
    fn from(fns: FlexibleNumeralString) -> Self {
        fns.0
    }
}

impl<R: RadixOps> NumeralString<R> for FlexibleNumeralString {
    fn is_valid(&self, radix: &R) -> bool {
        self.0
            .iter()
            .fold(true, |acc, n| acc && radix.check_in_range(*n as u32))
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn split(&self, u: usize) -> (Self, Self) {
        let mut front = self.0.clone();
        let back = front.split_off(u);
        (FlexibleNumeralString(front), FlexibleNumeralString(back))
    }

    fn concat(mut a: Self, mut b: Self) -> Self {
        a.0.append(&mut b.0);
        a
    }

    fn num_radix(&self, radix: &BigUint) -> BigUint {
        let mut res = BigUint::zero();
        for i in &self.0 {
            res *= radix;
            res += BigUint::from(*i);
        }
        res
    }

    fn str_radix(mut x: BigUint, radix: &BigUint, m: usize) -> Self {
        let mut res = vec![0; m];
        for i in 0..m {
            res[m - 1 - i] = (&x % radix).to_u16().unwrap();
            x = x / radix;
        }
        FlexibleNumeralString(res)
    }
}

impl RadixOps for u16 {
    fn check_in_range(&self, n: u32) -> bool {
        n % *self as u32 == n
    }

    fn calculate_b(&self, v: usize) -> usize {
        (v as f64 * (*self as f64).log2() / 8f64).ceil() as usize
    }

    fn to_biguint(&self) -> BigUint {
        BigUint::from(*self)
    }

    fn to_u32(&self) -> u32 {
        *self as u32
    }
}

/// A radix 2^i for i in [1..16). It does not use floating-point arithmetic.
#[derive(Debug, PartialEq)]
pub struct PowerTwoRadix {
    radix: u16,
    log_radix: u8,
}

impl PowerTwoRadix {
    pub fn from(radix: u16) -> Result<Self, ()> {
        let mut tmp = radix;
        let mut log_radix = 0;
        let mut found = false;
        for i in 0..16 {
            if tmp & 1 != 0 {
                // Only a single bit should be set
                if found {
                    return Err(());
                }
                log_radix = i;
                found = true;
            }
            tmp >>= 1;
        }
        match log_radix {
            0 => Err(()),
            _ => Ok(PowerTwoRadix { radix, log_radix }),
        }
    }
}

impl RadixOps for PowerTwoRadix {
    fn check_in_range(&self, n: u32) -> bool {
        n >> self.log_radix == 0
    }

    fn calculate_b(&self, v: usize) -> usize {
        ((v * self.log_radix as usize) + 7) / 8
    }

    fn to_biguint(&self) -> BigUint {
        BigUint::from(self.radix)
    }

    fn to_u32(&self) -> u32 {
        self.radix as u32
    }
}

fn pow(x: &BigUint, e: usize) -> BigUint {
    let mut res = BigUint::one();
    for _ in 0..e {
        res *= x;
    }
    res
}

fn generate_s<CIPH: BlockCipher>(ciph: &CIPH, r: &[u8], d: usize) -> Vec<u8> {
    let mut s = Vec::from(r);
    s.reserve(d);
    {
        let mut j = BigUint::one();
        while s.len() < d {
            let tmp = j.to_bytes_be();
            assert!(tmp.len() <= 16);
            let mut block = [0; 16];
            block[16 - tmp.len()..].copy_from_slice(&tmp);
            for k in 0..16 {
                block[k] ^= r[k];
            }
            ciph.encrypt_block(&mut GenericArray::from_mut_slice(&mut block));
            s.extend_from_slice(&block[..]);
            j += BigUint::one();
        }
    }
    s.truncate(d);
    s
}

pub struct FF1<CIPH: BlockCipher, R: RadixOps> {
    ciph: CIPH,
    radix: R,
    radix_bi: BigUint,
}

impl<CIPH: BlockCipher, R: RadixOps> FF1<CIPH, R> {
    fn prf(&self, x: &[u8]) -> [u8; 16] {
        let m = x.len() / 16;
        let mut y = [0u8; 16];
        for j in 0..m {
            for i in 0..16 {
                y[i] ^= x[j * 16 + i];
            }
            self.ciph
                .encrypt_block(&mut GenericArray::from_mut_slice(&mut y));
        }
        y
    }

    pub fn new(key: &[u8], radix: R) -> Self {
        let ciph = CIPH::new(GenericArray::from_slice(key));
        let radix_bi = radix.to_biguint();
        FF1 {
            ciph,
            radix,
            radix_bi,
        }
    }

    /// Encrypts the given numeral string.
    ///
    /// Returns an error if the numeral string is not in the required radix.
    pub fn encrypt<NS: NumeralString<R>>(&self, tweak: &[u8], x: &NS) -> Result<NS, ()> {
        if !x.is_valid(&self.radix) {
            return Err(());
        }

        let n = x.len();
        let t = tweak.len();

        // 1. Let u = floor(n / 2); v = n - u
        let u = n / 2;
        let v = n - u;

        // 2. Let A = X[1..u]; B = X[u + 1..n].
        let (mut x_a, mut x_b) = x.split(u);

        // 3. Let b = ceil(ceil(v * log2(radix)) / 8).
        let b = self.radix.calculate_b(v);

        // 4. Let d = 4 * ceil(b / 4) + 4.
        let d = 4 * ((b + 3) / 4) + 4;

        // 5. Let P = [1, 2, 1] || [radix] || [10] || [u mod 256] || [n] || [t].
        let mut p = vec![1, 2, 1];
        p.write_u24::<BigEndian>(self.radix.to_u32()).unwrap();
        p.write_u8(10).unwrap();
        p.write_u8(u as u8).unwrap();
        p.write_u32::<BigEndian>(n as u32).unwrap();
        p.write_u32::<BigEndian>(t as u32).unwrap();

        //  6i. Let Q = T || [0]^((-t-b-1) mod 16) || [i] || [NUM(B, radix)].
        let q_base = {
            let val = ((((-(t as i32) - (b as i32) - 1) % 16) + 16) % 16) as usize;
            let mut q = Vec::from(tweak);
            q.resize(t + val, 0);
            q
        };
        for i in 0..10 {
            let mut q = q_base.clone();
            q.write_u8(i).unwrap();
            let q_bytes = x_b.num_radix(&self.radix_bi).to_bytes_be();
            for _ in 0..(b - q_bytes.len()) {
                q.write_u8(0).unwrap();
            }
            q.extend(q_bytes);

            // 6ii. Let R = PRF(P || Q).
            let r = self.prf(&[&p[..], &q[..]].concat());

            // 6iii. Let S be the first d bytes of R.
            let s = generate_s(&self.ciph, &r[..], d);

            // 6iv. Let y = NUM(S).
            let y = BigUint::from_bytes_be(&s);

            // 6v. If i is even, let m = u; else, let m = v.
            let m = if i % 2 == 0 { u } else { v };

            // 6vi. Let c = (NUM(A, radix) + y) mod radix^m.
            let c = (x_a.num_radix(&self.radix_bi) + y) % pow(&self.radix_bi, m);

            // 6vii. Let C = STR(c, radix).
            let x_c = NS::str_radix(c, &self.radix_bi, m);

            // 6viii. Let A = B.
            x_a = x_b;

            // 6ix. Let B = C.
            x_b = x_c;
        }

        // 7. Return A || B.
        Ok(NS::concat(x_a, x_b))
    }

    /// Decrypts the given numeral string.
    ///
    /// Returns an error if the numeral string is not in the required radix.
    pub fn decrypt<NS: NumeralString<R>>(&self, tweak: &[u8], x: &NS) -> Result<NS, ()> {
        if !x.is_valid(&self.radix) {
            return Err(());
        }

        let n = x.len();
        let t = tweak.len();

        // 1. Let u = floor(n / 2); v = n - u
        let u = n / 2;
        let v = n - u;

        // 2. Let A = X[1..u]; B = X[u + 1..n].
        let (mut x_a, mut x_b) = x.split(u);

        // 3. Let b = ceil(ceil(v * log2(radix)) / 8).
        let b = self.radix.calculate_b(v);

        // 4. Let d = 4 * ceil(b / 4) + 4.
        let d = 4 * ((b + 3) / 4) + 4;

        // 5. Let P = [1, 2, 1] || [radix] || [10] || [u mod 256] || [n] || [t].
        let mut p = vec![1, 2, 1];
        p.write_u24::<BigEndian>(self.radix.to_u32()).unwrap();
        p.write_u8(10).unwrap();
        p.write_u8(u as u8).unwrap();
        p.write_u32::<BigEndian>(n as u32).unwrap();
        p.write_u32::<BigEndian>(t as u32).unwrap();

        //  6i. Let Q = T || [0]^((-t-b-1) mod 16) || [i] || [NUM(A, radix)].
        let q_base = {
            let val = ((((-(t as i32) - (b as i32) - 1) % 16) + 16) % 16) as usize;
            let mut q = Vec::from(tweak);
            q.resize(t + val, 0);
            q
        };
        for i in 0..10 {
            let i = 9 - i;
            let mut q = q_base.clone();
            q.write_u8(i).unwrap();
            let q_bytes = x_a.num_radix(&self.radix_bi).to_bytes_be();
            for _ in 0..(b - q_bytes.len()) {
                q.write_u8(0).unwrap();
            }
            q.extend(q_bytes);

            // 6ii. Let R = PRF(P || Q).
            let r = self.prf(&[&p[..], &q[..]].concat());

            // 6iii. Let S be the first d bytes of R.
            let s = generate_s(&self.ciph, &r[..], d);

            // 6iv. Let y = NUM(S).
            let y = BigInt::from(BigUint::from_bytes_be(&s));

            // 6v. If i is even, let m = u; else, let m = v.
            let m = if i % 2 == 0 { u } else { v };

            // 6vi. Let c = (NUM(B, radix) - y) mod radix^m.
            let modulus = BigInt::from(pow(&self.radix_bi, m));
            let mut c = (BigInt::from(x_b.num_radix(&self.radix_bi)) - y) % &modulus;
            if c.sign() == Sign::Minus {
                // use ((x % m) + m) % m to ensure it is in range
                c += &modulus;
                c %= modulus;
            }
            let c = c.to_biguint().unwrap();

            // 6vii. Let C = STR(c, radix).
            let x_c = NS::str_radix(c, &self.radix_bi, m);

            // 6viii. Let B = A.
            x_b = x_a;

            // 6ix. Let A = C.
            x_a = x_c;
        }

        // 7. Return A || B.
        Ok(NS::concat(x_a, x_b))
    }
}

#[cfg(test)]
mod tests {
    use aes::{Aes128, Aes192, Aes256};

    use super::{FF1, FlexibleNumeralString, NumeralString, PowerTwoRadix, RadixOps};

    #[test]
    fn val_in_range() {
        let radix = 10;
        for i in 0..10 {
            assert_eq!(radix.check_in_range(i), true);
        }
        for i in 10..20 {
            assert_eq!(radix.check_in_range(i), false);
        }
    }

    #[test]
    fn ns_is_valid() {
        let radix = 10;
        let ns = FlexibleNumeralString::from(vec![0, 5, 9]);
        assert!(ns.is_valid(&radix));

        let ns = FlexibleNumeralString::from(vec![0, 5, 10]);
        assert!(!ns.is_valid(&radix));
    }

    #[test]
    fn power_two_radix() {
        assert_eq!(PowerTwoRadix::from(1), Err(()));
        assert_eq!(
            PowerTwoRadix::from(2),
            Ok(PowerTwoRadix {
                radix: 2,
                log_radix: 1,
            })
        );
        assert_eq!(PowerTwoRadix::from(3), Err(()));
        assert_eq!(
            PowerTwoRadix::from(4),
            Ok(PowerTwoRadix {
                radix: 4,
                log_radix: 2,
            })
        );
        assert_eq!(PowerTwoRadix::from(5), Err(()));
        assert_eq!(PowerTwoRadix::from(6), Err(()));
        assert_eq!(PowerTwoRadix::from(7), Err(()));
        assert_eq!(
            PowerTwoRadix::from(8),
            Ok(PowerTwoRadix {
                radix: 8,
                log_radix: 3,
            })
        );
        assert_eq!(
            PowerTwoRadix::from(32768),
            Ok(PowerTwoRadix {
                radix: 32768,
                log_radix: 15,
            })
        );
    }

    #[test]
    fn test_vectors() {
        enum AesType {
            AES128,
            AES192,
            AES256,
        };

        struct TestVector {
            aes: AesType,
            key: Vec<u8>,
            radix: u16,
            tweak: Vec<u8>,
            pt: Vec<u16>,
            ct: Vec<u16>,
        };

        let test_vectors = vec![
            // From https://csrc.nist.gov/CSRC/media/Projects/Cryptographic-Standards-and-Guidelines/documents/examples/FF1samples.pdf
            TestVector {
                // Sample #1
                aes: AesType::AES128,
                key: vec![
                    0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6, 0xAB, 0xF7, 0x15, 0x88, 0x09,
                    0xCF, 0x4F, 0x3C,
                ],
                radix: 10,
                tweak: vec![],
                pt: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
                ct: vec![2, 4, 3, 3, 4, 7, 7, 4, 8, 4],
            },
            TestVector {
                // Sample #2
                aes: AesType::AES128,
                key: vec![
                    0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6, 0xAB, 0xF7, 0x15, 0x88, 0x09,
                    0xCF, 0x4F, 0x3C,
                ],
                radix: 10,
                tweak: vec![0x39, 0x38, 0x37, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31, 0x30],
                pt: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
                ct: vec![6, 1, 2, 4, 2, 0, 0, 7, 7, 3],
            },
            TestVector {
                // Sample #3
                aes: AesType::AES128,
                key: vec![
                    0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6, 0xAB, 0xF7, 0x15, 0x88, 0x09,
                    0xCF, 0x4F, 0x3C,
                ],
                radix: 36,
                tweak: vec![
                    0x37, 0x37, 0x37, 0x37, 0x70, 0x71, 0x72, 0x73, 0x37, 0x37, 0x37,
                ],
                pt: vec![
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
                ],
                ct: vec![
                    10, 9, 29, 31, 4, 0, 22, 21, 21, 9, 20, 13, 30, 5, 0, 9, 14, 30, 22,
                ],
            },
            TestVector {
                // Sample #4
                aes: AesType::AES192,
                key: vec![
                    0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6, 0xAB, 0xF7, 0x15, 0x88, 0x09,
                    0xCF, 0x4F, 0x3C, 0xEF, 0x43, 0x59, 0xD8, 0xD5, 0x80, 0xAA, 0x4F,
                ],
                radix: 10,
                tweak: vec![],
                pt: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
                ct: vec![2, 8, 3, 0, 6, 6, 8, 1, 3, 2],
            },
            TestVector {
                // Sample #5
                aes: AesType::AES192,
                key: vec![
                    0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6, 0xAB, 0xF7, 0x15, 0x88, 0x09,
                    0xCF, 0x4F, 0x3C, 0xEF, 0x43, 0x59, 0xD8, 0xD5, 0x80, 0xAA, 0x4F,
                ],
                radix: 10,
                tweak: vec![0x39, 0x38, 0x37, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31, 0x30],
                pt: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
                ct: vec![2, 4, 9, 6, 6, 5, 5, 5, 4, 9],
            },
            TestVector {
                // Sample #6
                aes: AesType::AES192,
                key: vec![
                    0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6, 0xAB, 0xF7, 0x15, 0x88, 0x09,
                    0xCF, 0x4F, 0x3C, 0xEF, 0x43, 0x59, 0xD8, 0xD5, 0x80, 0xAA, 0x4F,
                ],
                radix: 36,
                tweak: vec![
                    0x37, 0x37, 0x37, 0x37, 0x70, 0x71, 0x72, 0x73, 0x37, 0x37, 0x37,
                ],
                pt: vec![
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
                ],
                ct: vec![
                    33, 11, 19, 3, 20, 31, 3, 5, 19, 27, 10, 32, 33, 31, 3, 2, 34, 28, 27,
                ],
            },
            TestVector {
                // Sample #7
                aes: AesType::AES256,
                key: vec![
                    0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6, 0xAB, 0xF7, 0x15, 0x88, 0x09,
                    0xCF, 0x4F, 0x3C, 0xEF, 0x43, 0x59, 0xD8, 0xD5, 0x80, 0xAA, 0x4F, 0x7F, 0x03,
                    0x6D, 0x6F, 0x04, 0xFC, 0x6A, 0x94,
                ],
                radix: 10,
                tweak: vec![],
                pt: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
                ct: vec![6, 6, 5, 7, 6, 6, 7, 0, 0, 9],
            },
            TestVector {
                // Sample #8
                aes: AesType::AES256,
                key: vec![
                    0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6, 0xAB, 0xF7, 0x15, 0x88, 0x09,
                    0xCF, 0x4F, 0x3C, 0xEF, 0x43, 0x59, 0xD8, 0xD5, 0x80, 0xAA, 0x4F, 0x7F, 0x03,
                    0x6D, 0x6F, 0x04, 0xFC, 0x6A, 0x94,
                ],
                radix: 10,
                tweak: vec![0x39, 0x38, 0x37, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31, 0x30],
                pt: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
                ct: vec![1, 0, 0, 1, 6, 2, 3, 4, 6, 3],
            },
            TestVector {
                // Sample #9
                aes: AesType::AES256,
                key: vec![
                    0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6, 0xAB, 0xF7, 0x15, 0x88, 0x09,
                    0xCF, 0x4F, 0x3C, 0xEF, 0x43, 0x59, 0xD8, 0xD5, 0x80, 0xAA, 0x4F, 0x7F, 0x03,
                    0x6D, 0x6F, 0x04, 0xFC, 0x6A, 0x94,
                ],
                radix: 36,
                tweak: vec![
                    0x37, 0x37, 0x37, 0x37, 0x70, 0x71, 0x72, 0x73, 0x37, 0x37, 0x37,
                ],
                pt: vec![
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
                ],
                ct: vec![
                    33, 28, 8, 10, 0, 10, 35, 17, 2, 10, 31, 34, 10, 21, 34, 35, 30, 32, 13,
                ],
            },
            // From https://github.com/capitalone/fpe/blob/master/ff1/ff1_test.go
            TestVector {
                aes: AesType::AES256,
                key: vec![
                    0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6, 0xAB, 0xF7, 0x15, 0x88, 0x09,
                    0xCF, 0x4F, 0x3C, 0xEF, 0x43, 0x59, 0xD8, 0xD5, 0x80, 0xAA, 0x4F, 0x7F, 0x03,
                    0x6D, 0x6F, 0x04, 0xFC, 0x6A, 0x94,
                ],
                radix: 36,
                tweak: vec![],
                pt: vec![
                    33, 28, 8, 10, 0, 10, 35, 17, 2, 10, 31, 34, 10, 21, 34, 35, 30, 32, 13, 33,
                    28, 8, 10, 0, 10, 35, 17, 2, 10, 31, 34, 10, 21, 34, 35, 30, 32, 13, 33, 28, 8,
                    10, 0, 10, 35, 17, 2, 10, 31, 34, 10, 21, 34, 35, 30, 32, 13, 33, 28, 8, 10, 0,
                    10, 35, 17, 2, 10, 31, 34, 10, 21, 34, 35, 30, 32, 13, 33, 28, 8, 10, 0, 10,
                    35, 17, 2, 10, 31, 34, 10, 21, 34, 35, 30, 32, 13, 33, 28, 8, 10, 0, 10, 35,
                    17, 2, 10, 31, 34, 10, 21, 34, 35, 30, 32, 13, 33, 28, 8, 10, 0, 10, 35, 17, 2,
                    10, 31, 34, 10, 21,
                ],
                // lwulibfp1ju3ksztumqomwenpv7duy9q7pg7zf3eg3rjlfy46gmgkqjfwvromfjjktmbey8meqk9zkcmgvkv4s9ll5ctozme1hf15w7xo6zsylqcr0nbx9jbf10umzok
                ct: vec![
                    21, 32, 30, 21, 18, 11, 15, 25, 1, 19, 30, 3, 20, 28, 35, 29, 30, 22, 26, 24,
                    22, 32, 14, 23, 25, 31, 7, 13, 30, 34, 9, 26, 7, 25, 16, 7, 35, 15, 3, 14, 16,
                    3, 27, 19, 21, 15, 34, 4, 6, 16, 22, 16, 20, 26, 19, 15, 32, 31, 27, 24, 22,
                    15, 19, 19, 20, 29, 22, 11, 14, 34, 8, 22, 14, 26, 20, 9, 35, 20, 12, 22, 16,
                    31, 20, 31, 4, 28, 9, 21, 21, 5, 12, 29, 24, 35, 22, 14, 1, 17, 15, 1, 5, 32,
                    7, 33, 24, 6, 35, 28, 34, 21, 26, 12, 27, 0, 23, 11, 33, 9, 19, 11, 15, 1, 0,
                    30, 22, 35, 24, 20,
                ],
            },
            // Zcash test vectors
            TestVector {
                aes: AesType::AES256,
                key: vec![
                    0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6, 0xAB, 0xF7, 0x15, 0x88, 0x09,
                    0xCF, 0x4F, 0x3C, 0xEF, 0x43, 0x59, 0xD8, 0xD5, 0x80, 0xAA, 0x4F, 0x7F, 0x03,
                    0x6D, 0x6F, 0x04, 0xFC, 0x6A, 0x94,
                ],
                radix: 2,
                tweak: vec![],
                pt: vec![0; 88],
                ct: vec![
                    0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1, 0, 1, 0, 1, 0, 1, 1, 1, 0, 1, 1, 1, 1, 1,
                    1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 0, 0, 1, 1, 0, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0,
                    0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 0, 1, 0, 1, 1, 0, 1, 0, 1, 0, 0, 1, 0, 0, 0, 1,
                    0, 0, 1, 1, 0, 0, 1, 1, 1, 1,
                ],
            },
            TestVector {
                aes: AesType::AES256,
                key: vec![
                    0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6, 0xAB, 0xF7, 0x15, 0x88, 0x09,
                    0xCF, 0x4F, 0x3C, 0xEF, 0x43, 0x59, 0xD8, 0xD5, 0x80, 0xAA, 0x4F, 0x7F, 0x03,
                    0x6D, 0x6F, 0x04, 0xFC, 0x6A, 0x94,
                ],
                radix: 2,
                tweak: vec![],
                pt: vec![
                    0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1, 0, 1, 0, 1, 0, 1, 1, 1, 0, 1, 1, 1, 1, 1,
                    1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 0, 0, 1, 1, 0, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0,
                    0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 0, 1, 0, 1, 1, 0, 1, 0, 1, 0, 0, 1, 0, 0, 0, 1,
                    0, 0, 1, 1, 0, 0, 1, 1, 1, 1,
                ],
                ct: vec![
                    1, 1, 0, 1, 1, 0, 1, 0, 1, 1, 0, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 1, 1, 1, 0, 0,
                    0, 0, 0, 1, 0, 0, 1, 1, 0, 0, 1, 1, 1, 1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1,
                    0, 1, 0, 1, 1, 0, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 0, 1, 0, 1, 0, 1,
                    1, 1, 1, 1, 0, 1, 1, 0, 0, 0,
                ],
            },
        ];

        for tv in test_vectors {
            let (ct, pt) = match tv.aes {
                AesType::AES128 => {
                    let ff = FF1::<Aes128, u16>::new(&tv.key, tv.radix);
                    (
                        ff.encrypt(&tv.tweak, &FlexibleNumeralString::from(tv.pt.clone())),
                        ff.decrypt(&tv.tweak, &FlexibleNumeralString::from(tv.ct.clone())),
                    )
                }
                AesType::AES192 => {
                    let ff = FF1::<Aes192, u16>::new(&tv.key, tv.radix);
                    (
                        ff.encrypt(&tv.tweak, &FlexibleNumeralString::from(tv.pt.clone())),
                        ff.decrypt(&tv.tweak, &FlexibleNumeralString::from(tv.ct.clone())),
                    )
                }
                AesType::AES256 => {
                    let ff = FF1::<Aes256, u16>::new(&tv.key, tv.radix);
                    (
                        ff.encrypt(&tv.tweak, &FlexibleNumeralString::from(tv.pt.clone())),
                        ff.decrypt(&tv.tweak, &FlexibleNumeralString::from(tv.ct.clone())),
                    )
                }
            };
            assert_eq!(Vec::from(ct.unwrap()), tv.ct);
            assert_eq!(Vec::from(pt.unwrap()), tv.pt);
        }
    }

    #[test]
    fn test_vectors_radix_power_2() {
        struct TestVector {
            key: Vec<u8>,
            radix: u16,
            tweak: Vec<u8>,
            pt: Vec<u16>,
            ct: Vec<u16>,
        };

        let test_vectors = vec![
            // Zcash test vectors
            TestVector {
                key: vec![
                    0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6, 0xAB, 0xF7, 0x15, 0x88, 0x09,
                    0xCF, 0x4F, 0x3C, 0xEF, 0x43, 0x59, 0xD8, 0xD5, 0x80, 0xAA, 0x4F, 0x7F, 0x03,
                    0x6D, 0x6F, 0x04, 0xFC, 0x6A, 0x94,
                ],
                radix: 2,
                tweak: vec![],
                pt: vec![0; 88],
                ct: vec![
                    0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1, 0, 1, 0, 1, 0, 1, 1, 1, 0, 1, 1, 1, 1, 1,
                    1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 0, 0, 1, 1, 0, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0,
                    0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 0, 1, 0, 1, 1, 0, 1, 0, 1, 0, 0, 1, 0, 0, 0, 1,
                    0, 0, 1, 1, 0, 0, 1, 1, 1, 1,
                ],
            },
            TestVector {
                key: vec![
                    0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6, 0xAB, 0xF7, 0x15, 0x88, 0x09,
                    0xCF, 0x4F, 0x3C, 0xEF, 0x43, 0x59, 0xD8, 0xD5, 0x80, 0xAA, 0x4F, 0x7F, 0x03,
                    0x6D, 0x6F, 0x04, 0xFC, 0x6A, 0x94,
                ],
                radix: 2,
                tweak: vec![],
                pt: vec![
                    0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1, 0, 1, 0, 1, 0, 1, 1, 1, 0, 1, 1, 1, 1, 1,
                    1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 0, 0, 1, 1, 0, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0,
                    0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 0, 1, 0, 1, 1, 0, 1, 0, 1, 0, 0, 1, 0, 0, 0, 1,
                    0, 0, 1, 1, 0, 0, 1, 1, 1, 1,
                ],
                ct: vec![
                    1, 1, 0, 1, 1, 0, 1, 0, 1, 1, 0, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 1, 1, 1, 0, 0,
                    0, 0, 0, 1, 0, 0, 1, 1, 0, 0, 1, 1, 1, 1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1,
                    0, 1, 0, 1, 1, 0, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 0, 1, 0, 1, 0, 1,
                    1, 1, 1, 1, 0, 1, 1, 0, 0, 0,
                ],
            },
        ];

        for tv in test_vectors {
            let (ct, pt) = {
                let ff = FF1::<Aes256, PowerTwoRadix>::new(
                    &tv.key,
                    PowerTwoRadix::from(tv.radix).unwrap(),
                );
                (
                    ff.encrypt(&tv.tweak, &FlexibleNumeralString::from(tv.pt.clone())),
                    ff.decrypt(&tv.tweak, &FlexibleNumeralString::from(tv.ct.clone())),
                )
            };
            assert_eq!(Vec::from(ct.unwrap()), tv.ct);
            assert_eq!(Vec::from(pt.unwrap()), tv.pt);
        }
    }
}
