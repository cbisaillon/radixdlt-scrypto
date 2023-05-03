use strum::FromRepr;

//=========================================================================
// Please update REP-60 after updating types/configs defined in this file!
// Please use and update REP-71 for choosing an entity type prefix
//=========================================================================

/// An enum which represents the different addressable entities.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd, FromRepr)]
pub enum EntityType {
    //=========================================================================
    // Package (start with char p for package)
    //=========================================================================
    /// A global package entity (13 in decimal). Gives Bech32 prefix: `p` followed by one of `5`, `4`, `k` or `h`.
    GlobalPackage = 0b00001101, //------------------- 00001 => p, 101xx => 54kh [pkg vanity prefix]

    //=========================================================================
    // System Components (start with char s for system)
    //=========================================================================
    /// The global epoch manager entity (134 in decimal). Gives Bech32 prefix: `s` followed by one of `c`, `e`, `6` or `m`.
    GlobalEpochManager = 0b10000110, //-------------- 10000 => s, 110xx => ce6m [se vanity prefix]

    /// A global validator entity (130 in decimal). Gives Bech32 prefix: `s` followed by one of `v`, `d`, `w` or `0`.
    GlobalValidator = 0b10000010, //----------------- 10000 => s, 110xx => vdw0

    /// The global clock entity (133 in decimal). Gives Bech32 prefix: `s` followed by one of `5`, `4`, `k` or `h`.
    GlobalClock = 0b10000101, //--------------------- 10000 => s, 101xx => 54kh

    //=========================================================================
    // Standard Global Components (start with char c for component)
    //=========================================================================
    /// A global generic (eg scrypto) component entity (192 in decimal). Gives Bech32 prefix: `c` followed by one of `q`, `p`, `z` or `r`.
    GlobalGenericComponent = 0b11000000, //---------- 11000 => c, 000xx => qpzr [cpt vanity prefix] (000 = generic component)

    /// A global non-virtual native account component entity (193 in decimal). Gives Bech32 prefix: `c` followed by one of `y`, `9`, `x` or `8`.
    GlobalAccount = 0b11000001, //------------------- 11000 => c, 001xx => y9x8 (001 = account)

    /// A global non-virtual native identity component entity (194 in decimal). Gives Bech32 prefix: `c` followed by one of `g`, `f`, `2` or `t`.
    GlobalIdentity = 0b11000010, //------------------ 11000 => c, 010xx => gf2t (010 = identity)

    /// A global native access controller entity (195 in decimal). Gives Bech32 prefix: `c` followed by one of `v`, `d`, `w` or `0`.
    GlobalAccessController = 0b11000011, //---------- 11000 => c, 011xx => vdw0 (011 = access controller)

    //=========================================================================
    // Secp256k1 Virtual Global Components (start with char 6 for Secp256k1)
    //=========================================================================
    /// A global virtual Secp256k1 account component entity (209 in decimal). Gives Bech32 prefix: `6` followed by one of `y`, `9`, `x` or `8`.
    GlobalVirtualSecp256k1Account = 0b11010001, //--- 11010 => 6, 001xx => y9x8 (001 = account)

    /// A global virtual Secp256k1 identity component entity (210 in decimal). Gives Bech32 prefix: `6` followed by one of `g`, `f`, `2` or `t`.
    GlobalVirtualSecp256k1Identity = 0b11010010, //-- 11010 => 6, 010xx => gf2t (010 = identity)

    //=========================================================================
    // Ed25519 Virtual Global Components (start with char 2 for Ed25519)
    //=========================================================================
    /// A global virtual Ed25519 account component entity (81 in decimal). Gives Bech32 prefix: `2` followed by one of `y`, `9`, `x` or `8`.
    GlobalVirtualEd25519Account = 0b01010001, //----- 01010 => 2, 001xx => y9x8 (001 = account)

    /// A global virtual Ed25519 identity component entity (82 in decimal). Gives Bech32 prefix: `2` followed by one of `g`, `f`, `2` or `t`.
    GlobalVirtualEd25519Identity = 0b01010010, //---- 01010 => 2, 010xx => gf2t (010 = identity)

    //=========================================================================
    // Fungible-related (start with letter t for token)
    //=========================================================================
    /// A global fungible resource entity (93 in decimal). Gives Bech32 prefix: `t` followed by one of `5`, `4`, `k` or `h`.
    GlobalFungibleResource = 0b01011101, //---------- 01011 => t, 101xx => 54kh [tkn vanity prefix]
    /// An internal fungible vault entity (88 in decimal). Gives Bech32 prefix: `t` followed by one of `q`, `p`, `z` or `r`.
    InternalFungibleVault = 0b01011000, //----------- 01011 => t, 000xx => qpzr (000 = vault under t/f prefix)

    //=========================================================================
    // Non-fungible-related (start with letter n for non-fungible)
    //=========================================================================
    /// A global non-fungible resource entity (154 in decimal). Gives Bech32 prefix: `n` followed by one of `g`, `f`, `2` or `t`.
    GlobalNonFungibleResource = 0b10011010, //------- 10011 => n, 010xx => gf2t [nf  vanity prefix]

    /// An internal non-fungible vault entity (152 in decimal). Gives Bech32 prefix: `n` followed by one of `q`, `p`, `z` or `r`.
    InternalNonFungibleVault = 0b10011000, //-------- 10011 => n, 000xx => qpzr (000 = vault under t/f prefix)

    //=========================================================================
    // Internal misc components (start with letter l for ..? local)
    //=========================================================================
    /// An internal generic (eg scrypto) component entity (248 in decimal). Gives Bech32 prefix: `l` followed by one of `q`, `p`, `z` or `r`.
    InternalGenericComponent = 0b11111000, //-------- 11111 => l, 000xx => qpzr (000 = generic component)

    /// An internal non-virtual native account component entity (249 in decimal). Gives Bech32 prefix: `l` followed by one of `y`, `9`, `x` or `8`.
    InternalAccount = 0b11111001, //----------------- 11111 => l, 001xx => y9x8 (001 = account)

    //=========================================================================
    // Internal key-value-store-like entities (start with k for key-value)
    //=========================================================================
    /// An internal key-value store entity (176 in decimal). Gives Bech32 prefix: `k` followed by one of `q`, `p`, `z` or `r`.
    ///
    /// A key value store allows access to substates, but not on-ledger iteration.
    /// The substates are considered independent for contention/locking/versioning.
    InternalKeyValueStore = 0b10110000, //----------- 10110 => k, 000xx => qpzr

    /// An internal index entity (177 in decimal). Gives Bech32 prefix: `k` followed by one of `q`, `p`, `z` or `r`.
    ///
    /// An index allows access to substates, and on-ledger iteration (ordered by key hash).
    /// The whole index is considered a single unit for contention/locking/versioning.
    InternalIndex = 0b10110001, //------------------- 10110 => k, 001xx => y9x8

    /// An internal sorted index entity (178 in decimal). Gives Bech32 prefix: `k` followed by one of `q`, `p`, `z` or `r`.
    ///
    /// An index allows access to substates, and on-ledger iteration (ordered by `prefix_u16 || key_hash`).
    /// The whole index is considered a single unit for contention/locking/versioning.
    InternalSortedIndex = 0b10110010, //------------- 10110 => k, 010xx => gf2t
}

impl EntityType {
    pub const fn is_global(&self) -> bool {
        match self {
            EntityType::GlobalPackage
            | EntityType::GlobalFungibleResource
            | EntityType::GlobalNonFungibleResource
            | EntityType::GlobalEpochManager
            | EntityType::GlobalValidator
            | EntityType::GlobalClock
            | EntityType::GlobalAccessController
            | EntityType::GlobalAccount
            | EntityType::GlobalIdentity
            | EntityType::GlobalGenericComponent
            | EntityType::GlobalVirtualSecp256k1Account
            | EntityType::GlobalVirtualEd25519Account
            | EntityType::GlobalVirtualSecp256k1Identity
            | EntityType::GlobalVirtualEd25519Identity => true,
            EntityType::InternalFungibleVault
            | EntityType::InternalNonFungibleVault
            | EntityType::InternalAccount
            | EntityType::InternalGenericComponent
            | EntityType::InternalKeyValueStore
            | EntityType::InternalIndex
            | EntityType::InternalSortedIndex => false,
        }
    }

    pub const fn is_internal(&self) -> bool {
        !self.is_global()
    }

    pub const fn is_global_component(&self) -> bool {
        match self {
            EntityType::GlobalEpochManager
            | EntityType::GlobalValidator
            | EntityType::GlobalClock
            | EntityType::GlobalAccessController
            | EntityType::GlobalAccount
            | EntityType::GlobalIdentity
            | EntityType::GlobalGenericComponent
            | EntityType::GlobalVirtualSecp256k1Account
            | EntityType::GlobalVirtualEd25519Account
            | EntityType::GlobalVirtualSecp256k1Identity
            | EntityType::GlobalVirtualEd25519Identity => true,
            EntityType::GlobalPackage
            | EntityType::GlobalFungibleResource
            | EntityType::GlobalNonFungibleResource
            | EntityType::InternalFungibleVault
            | EntityType::InternalNonFungibleVault
            | EntityType::InternalAccount
            | EntityType::InternalGenericComponent
            | EntityType::InternalKeyValueStore
            | EntityType::InternalIndex
            | EntityType::InternalSortedIndex => false,
        }
    }

    pub const fn is_global_package(&self) -> bool {
        matches!(self, EntityType::GlobalPackage)
    }

    pub const fn is_global_resource(&self) -> bool {
        matches!(
            self,
            EntityType::GlobalFungibleResource | EntityType::GlobalNonFungibleResource
        )
    }

    pub const fn is_global_virtual(&self) -> bool {
        match self {
            EntityType::GlobalVirtualSecp256k1Account
            | EntityType::GlobalVirtualEd25519Account
            | EntityType::GlobalVirtualSecp256k1Identity
            | EntityType::GlobalVirtualEd25519Identity => true,
            _ => false,
        }
    }

    pub const fn is_global_fungible_resource(&self) -> bool {
        matches!(self, EntityType::GlobalFungibleResource)
    }

    pub const fn is_internal_kv_store(&self) -> bool {
        matches!(self, EntityType::InternalKeyValueStore)
    }

    pub const fn is_internal_fungible_vault(&self) -> bool {
        matches!(self, EntityType::InternalFungibleVault)
    }

    pub const fn is_internal_vault(&self) -> bool {
        matches!(
            self,
            EntityType::InternalFungibleVault | EntityType::InternalNonFungibleVault
        )
    }
}