//! BEP52 (BitTorrent v2) per-file Merkle trees.
//!
//! Leaves are SHA-256 hashes of 16 KiB blocks. Each `piece_length` chunk forms a
//! sub-tree whose root is a "piece layer" hash; the per-piece roots form the upper
//! tree whose root is the file's `pieces root`. Padding uses zero hashes, matching
//! the BEP52 reference creator.

use crate::sha256::sha256_hash;
use sha2::{Digest, Sha256};

/// Size of a Merkle leaf block (16 KiB), fixed by BEP52.
pub const BLOCK_SIZE: usize = 16 * 1024;

fn hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

/// Root of a balanced binary Merkle tree over a power-of-two number of leaf hashes.
///
/// Each parent is `SHA-256(left || right)`. The leaf count must be a power of two.
pub fn merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    assert!(!leaves.is_empty(), "merkle tree needs at least one leaf");
    assert!(
        leaves.len().is_power_of_two(),
        "merkle leaf count must be a power of two"
    );
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    while level.len() > 1 {
        level = level
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| hash_pair(&pair[0], &pair[1]))
            .collect();
    }
    level[0]
}

/// A file's BEP52 Merkle tree summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleTree {
    /// Root hash of the whole file (the `pieces root` stored in the file tree).
    pub piece_root: [u8; 32],
    /// Per-piece root hashes, one per `piece_length` chunk. These are the hashes
    /// concatenated into the `piece layers` field (for files larger than a piece).
    pub piece_layers: Vec<[u8; 32]>,
}

/// Compute the BEP52 Merkle tree for `data` split into `piece_length` pieces.
///
/// `piece_length` must be a power of two and at least [`BLOCK_SIZE`]. An empty
/// file yields a zero root and no piece layers (empty files carry no pieces root).
pub fn compute_merkle_tree(data: &[u8], piece_length: usize) -> MerkleTree {
    assert!(piece_length >= BLOCK_SIZE, "piece length must be >= 16 KiB");
    assert!(
        piece_length.is_power_of_two(),
        "piece length must be a power of two"
    );
    if data.is_empty() {
        return MerkleTree {
            piece_root: [0u8; 32],
            piece_layers: Vec::new(),
        };
    }

    let blocks_per_piece = piece_length / BLOCK_SIZE;
    let single_piece = data.len() <= piece_length;
    let zero = [0u8; 32];

    let mut piece_roots: Vec<[u8; 32]> = Vec::new();
    for piece in data.chunks(piece_length) {
        let mut leaves: Vec<[u8; 32]> = piece.chunks(BLOCK_SIZE).map(sha256_hash).collect();
        // A file smaller than one piece pads to the next power of two of its block
        // count; every other (partial) piece pads to the full piece boundary.
        let target = if single_piece {
            leaves.len().next_power_of_two()
        } else {
            blocks_per_piece
        };
        leaves.resize(target, zero);
        piece_roots.push(merkle_root(&leaves));
    }

    let piece_layers = piece_roots.clone();

    // Upper tree over the per-piece roots, balanced with the root of an all-zero piece.
    let pad_piece_hash = merkle_root(&vec![zero; blocks_per_piece]);
    piece_roots.resize(piece_roots.len().next_power_of_two(), pad_piece_hash);
    let piece_root = merkle_root(&piece_roots);

    MerkleTree {
        piece_root,
        piece_layers,
    }
}

/// Compute the BEP52 Merkle root of a single piece's data.
///
/// `single_piece_file` selects the padding rule: a file that fits in one piece
/// pads its block count to the next power of two, while a piece of a larger file
/// pads to the full `piece_length` block boundary. Used to verify a received
/// piece against its expected piece-layer hash without reading the whole file.
pub fn piece_root(data: &[u8], piece_length: usize, single_piece_file: bool) -> [u8; 32] {
    assert!(piece_length >= BLOCK_SIZE, "piece length must be >= 16 KiB");
    assert!(
        piece_length.is_power_of_two(),
        "piece length must be a power of two"
    );
    let blocks_per_piece = piece_length / BLOCK_SIZE;
    let mut leaves: Vec<[u8; 32]> = data.chunks(BLOCK_SIZE).map(sha256_hash).collect();
    let target = if single_piece_file {
        leaves.len().next_power_of_two().max(1)
    } else {
        blocks_per_piece
    };
    leaves.resize(target, [0u8; 32]);
    merkle_root(&leaves)
}

/// Extract the Merkle proof (sibling hashes, leaf level upward) for the leaf at
/// `index` in a power-of-two leaf set. Used to prove a single 16 KiB block
/// against a piece root without transmitting the whole piece.
pub fn merkle_proof(leaves: &[[u8; 32]], index: usize) -> Vec<[u8; 32]> {
    assert!(!leaves.is_empty(), "merkle tree needs at least one leaf");
    assert!(
        leaves.len().is_power_of_two(),
        "merkle leaf count must be a power of two"
    );
    assert!(index < leaves.len(), "leaf index out of range");
    let mut proof = Vec::new();
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    let mut idx = index;
    while level.len() > 1 {
        proof.push(level[idx ^ 1]);
        level = level
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| hash_pair(&pair[0], &pair[1]))
            .collect();
        idx >>= 1;
    }
    proof
}

/// Verify a single 16 KiB `block` against `root` using a sibling `proof` and the
/// block's `index` within the piece (BEP52 merkle proof). The index bits select
/// the left/right ordering at each level. Returns false on any mismatch.
pub fn verify_block_proof(block: &[u8], index: usize, proof: &[[u8; 32]], root: &[u8; 32]) -> bool {
    let mut hash = sha256_hash(block);
    let mut idx = index;
    for sibling in proof {
        hash = if idx & 1 == 0 {
            hash_pair(&hash, sibling)
        } else {
            hash_pair(sibling, &hash)
        };
        idx >>= 1;
    }
    &hash == root
}

/// Verify a whole file's data against an expected BEP52 Merkle tree (the
/// `pieces root` plus per-piece `piece layers`). Recomputes the tree and compares
/// both the root and every layer hash. Used by v2 recheck. Invalid piece lengths
/// yield `false` rather than panicking.
pub fn verify_merkle_tree(data: &[u8], piece_length: usize, expected: &MerkleTree) -> bool {
    if piece_length < BLOCK_SIZE || !piece_length.is_power_of_two() {
        return false;
    }
    compute_merkle_tree(data, piece_length) == *expected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(byte: u8) -> Vec<u8> {
        vec![byte; BLOCK_SIZE]
    }

    #[test]
    fn merkle_root_single_leaf_is_the_leaf() {
        let leaf = sha256_hash(b"leaf");
        assert_eq!(merkle_root(&[leaf]), leaf);
    }

    #[test]
    fn merkle_root_pairs_adjacent_hashes() {
        let a = sha256_hash(b"a");
        let b = sha256_hash(b"b");
        let c = sha256_hash(b"c");
        let d = sha256_hash(b"d");
        let ab = hash_pair(&a, &b);
        let cd = hash_pair(&c, &d);
        assert_eq!(merkle_root(&[a, b, c, d]), hash_pair(&ab, &cd));
    }

    #[test]
    #[should_panic(expected = "power of two")]
    fn merkle_root_rejects_non_power_of_two() {
        let leaf = [0u8; 32];
        merkle_root(&[leaf, leaf, leaf]);
    }

    #[test]
    fn empty_file_has_zero_root() {
        let tree = compute_merkle_tree(&[], BLOCK_SIZE);
        assert_eq!(tree.piece_root, [0u8; 32]);
        assert!(tree.piece_layers.is_empty());
    }

    #[test]
    fn single_block_file_root_is_the_block_hash() {
        let data = block(0x11);
        let tree = compute_merkle_tree(&data, BLOCK_SIZE);
        let h = sha256_hash(&data);
        assert_eq!(tree.piece_root, h);
        assert_eq!(tree.piece_layers, vec![h]);
    }

    #[test]
    fn small_file_pads_to_power_of_two_not_piece_boundary() {
        // 2 blocks, piece length 4 blocks: must pad to 2 (next pow2), NOT 4.
        let data = [block(0x01), block(0x02)].concat();
        let tree = compute_merkle_tree(&data, 4 * BLOCK_SIZE);
        let h0 = sha256_hash(&data[..BLOCK_SIZE]);
        let h1 = sha256_hash(&data[BLOCK_SIZE..]);
        let expected = hash_pair(&h0, &h1);
        assert_eq!(tree.piece_root, expected);
        assert_eq!(tree.piece_layers, vec![expected]);
    }

    #[test]
    fn partial_piece_pads_with_zero_hashes() {
        // 3 blocks in a single piece of 4 blocks -> pad to 4 with one zero hash.
        let data = [block(0x01), block(0x02), block(0x03)].concat();
        let tree = compute_merkle_tree(&data, 4 * BLOCK_SIZE);
        let h0 = sha256_hash(&data[..BLOCK_SIZE]);
        let h1 = sha256_hash(&data[BLOCK_SIZE..2 * BLOCK_SIZE]);
        let h2 = sha256_hash(&data[2 * BLOCK_SIZE..]);
        let zero = [0u8; 32];
        let expected = hash_pair(&hash_pair(&h0, &h1), &hash_pair(&h2, &zero));
        assert_eq!(tree.piece_root, expected);
    }

    #[test]
    fn multi_piece_file_builds_two_level_tree() {
        // 5 blocks, piece length 4 blocks: piece0 full (4), piece1 partial (1) padded to 4.
        let data = [
            block(0x01),
            block(0x02),
            block(0x03),
            block(0x04),
            block(0x05),
        ]
        .concat();
        let tree = compute_merkle_tree(&data, 4 * BLOCK_SIZE);
        let h: Vec<[u8; 32]> = (0..5)
            .map(|i| sha256_hash(&data[i * BLOCK_SIZE..(i + 1) * BLOCK_SIZE]))
            .collect();
        let zero = [0u8; 32];
        let root0 = hash_pair(&hash_pair(&h[0], &h[1]), &hash_pair(&h[2], &h[3]));
        let root1 = hash_pair(&hash_pair(&h[4], &zero), &hash_pair(&zero, &zero));
        assert_eq!(tree.piece_layers, vec![root0, root1]);
        assert_eq!(tree.piece_root, hash_pair(&root0, &root1));
    }

    #[test]
    fn full_multi_piece_file_matches_flat_tree() {
        // 8 blocks, piece length 4 blocks: two full pieces, upper tree of 2 roots.
        let data: Vec<u8> = (0..8).flat_map(|i| block(i as u8)).collect();
        let tree = compute_merkle_tree(&data, 4 * BLOCK_SIZE);
        let h: Vec<[u8; 32]> = (0..8)
            .map(|i| sha256_hash(&data[i * BLOCK_SIZE..(i + 1) * BLOCK_SIZE]))
            .collect();
        assert_eq!(tree.piece_root, merkle_root(&h));
    }

    #[test]
    fn merkle_proof_verifies_every_block() {
        let data: Vec<u8> = (0..8).flat_map(|i| block(i as u8)).collect();
        let leaves: Vec<[u8; 32]> = data.chunks(BLOCK_SIZE).map(sha256_hash).collect();
        let root = merkle_root(&leaves);
        for (i, leaf_block) in data.chunks(BLOCK_SIZE).enumerate() {
            let proof = merkle_proof(&leaves, i);
            assert!(
                verify_block_proof(leaf_block, i, &proof, &root),
                "block {i} must verify"
            );
        }
    }

    #[test]
    fn merkle_proof_single_leaf_has_empty_proof() {
        let data = block(0x11);
        let leaf = sha256_hash(&data);
        let proof = merkle_proof(&[leaf], 0);
        assert!(proof.is_empty());
        assert!(verify_block_proof(&data, 0, &proof, &leaf));
    }

    #[test]
    fn merkle_proof_rejects_tampered_block() {
        let data = block(0x42);
        let leaves = [sha256_hash(&data)];
        let root = merkle_root(&leaves);
        let proof = merkle_proof(&leaves, 0);
        let mut tampered = data.clone();
        tampered[0] ^= 0xff;
        assert!(!verify_block_proof(&tampered, 0, &proof, &root));
    }

    #[test]
    fn merkle_proof_rejects_wrong_index() {
        let data: Vec<u8> = (0..4).flat_map(|i| block(i as u8)).collect();
        let leaves: Vec<[u8; 32]> = data.chunks(BLOCK_SIZE).map(sha256_hash).collect();
        let root = merkle_root(&leaves);
        let proof = merkle_proof(&leaves, 0);
        assert!(!verify_block_proof(
            &data[BLOCK_SIZE..2 * BLOCK_SIZE],
            1,
            &proof,
            &root
        ));
    }

    #[test]
    fn verify_merkle_tree_accepts_matching_file() {
        let data: Vec<u8> = (0..5).flat_map(|i| block(i as u8)).collect();
        let tree = compute_merkle_tree(&data, 4 * BLOCK_SIZE);
        assert!(verify_merkle_tree(&data, 4 * BLOCK_SIZE, &tree));
    }

    #[test]
    fn verify_merkle_tree_rejects_tampered_file() {
        let data: Vec<u8> = (0..5).flat_map(|i| block(i as u8)).collect();
        let tree = compute_merkle_tree(&data, 4 * BLOCK_SIZE);
        let mut tampered = data.clone();
        tampered[BLOCK_SIZE] ^= 0xff;
        assert!(!verify_merkle_tree(&tampered, 4 * BLOCK_SIZE, &tree));
    }

    #[test]
    fn verify_merkle_tree_rejects_invalid_piece_length() {
        let data = block(0x11);
        let tree = compute_merkle_tree(&data, BLOCK_SIZE);
        assert!(!verify_merkle_tree(&data, 3 * BLOCK_SIZE, &tree));
        assert!(!verify_merkle_tree(&data, 8 * 1024, &tree));
    }

    #[test]
    fn piece_root_matches_multi_piece_layers() {
        // 5 blocks, piece length 4 blocks: piece0 full, piece1 partial.
        let data: Vec<u8> = (0..5).flat_map(|i| block(i as u8)).collect();
        let pl = 4 * BLOCK_SIZE;
        let tree = compute_merkle_tree(&data, pl);
        for (i, piece) in data.chunks(pl).enumerate() {
            assert_eq!(
                piece_root(piece, pl, false),
                tree.piece_layers[i],
                "piece {i} root must match its layer hash"
            );
        }
    }

    #[test]
    fn piece_root_matches_single_piece_file() {
        // A file smaller than a piece pads to the next power of two, not the
        // full piece boundary. Two blocks pad to 2 (single-piece rule) vs 4
        // (multi-piece rule), so the two roots differ.
        let data = [block(0x01), block(0x02)].concat();
        let pl = 4 * BLOCK_SIZE;
        let tree = compute_merkle_tree(&data, pl);
        assert_eq!(piece_root(&data, pl, true), tree.piece_root);
        // The multi-piece padding rule would give a different (wrong) root.
        assert_ne!(piece_root(&data, pl, false), tree.piece_root);
    }

    fn hex32(s: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    /// Known-answer vectors generated by an independent BEP52 reference
    /// implementation (block `i` filled with byte `i + 1`). These pin the
    /// padding rules and two-level tree against the spec, not against ourselves.
    #[test]
    fn bep52_reference_vectors() {
        let pl = 4 * BLOCK_SIZE;

        let one = compute_merkle_tree(&block(0x01), BLOCK_SIZE);
        assert_eq!(
            one.piece_root,
            hex32("111ce3c2a38d83a2e4706bde4abddd509d7f8248116c6832b06745bdc349e09f")
        );

        let two = compute_merkle_tree(&[block(0x01), block(0x02)].concat(), pl);
        assert_eq!(
            two.piece_root,
            hex32("1a1fb8144cac6f79b4f40c57031dc35f63b3c3f9fbe828413bc0d9210865d72a")
        );

        let three = compute_merkle_tree(&[block(0x01), block(0x02), block(0x03)].concat(), pl);
        assert_eq!(
            three.piece_root,
            hex32("35754ee03360cacab5e9e27647da3d2a4ab6f97614ed7d1692b9eb3c42a65d69")
        );

        let five_data: Vec<u8> = (1..=5).flat_map(|i| block(i as u8)).collect();
        let five = compute_merkle_tree(&five_data, pl);
        assert_eq!(
            five.piece_root,
            hex32("c5c907dc04430ebb1cbe748195971d38eb707c0b5a24e2ec6b5fd5c995cb6d7f")
        );
        assert_eq!(
            five.piece_layers,
            vec![
                hex32("8aa46b99f2907a2e06a12eed5ab6fcd5bd86bc7f49fe062eeaa78a4304b314eb"),
                hex32("12f841ef79e2ee312ee605056b70a9a965f3e5b65a6bcfbacfe8368be3548335"),
            ]
        );
        // Per-piece verification reproduces each reference layer hash.
        for (i, piece) in five_data.chunks(pl).enumerate() {
            assert_eq!(piece_root(piece, pl, false), five.piece_layers[i]);
        }

        let eight_data: Vec<u8> = (1..=8).flat_map(|i| block(i as u8)).collect();
        let eight = compute_merkle_tree(&eight_data, pl);
        assert_eq!(
            eight.piece_root,
            hex32("e2ae32815ef14871be7af2929c2bcceb3aea5ed5f3634502bff695150e29e53f")
        );
        assert_eq!(
            eight.piece_layers,
            vec![
                hex32("8aa46b99f2907a2e06a12eed5ab6fcd5bd86bc7f49fe062eeaa78a4304b314eb"),
                hex32("e350c93932fd0f4fc72834a7ed30aa3001a0ca8e31987fc1481fbe78244aec85"),
            ]
        );

        let empty = compute_merkle_tree(&[], BLOCK_SIZE);
        assert_eq!(empty.piece_root, [0u8; 32]);
        assert!(empty.piece_layers.is_empty());
    }
}
