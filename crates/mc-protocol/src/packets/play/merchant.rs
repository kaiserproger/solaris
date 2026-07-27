use super::*;

const MAX_MERCHANT_OFFERS: usize = 256;
const MAX_MERCHANT_COST_COMPONENTS: usize = 64;

/// One required input of a merchant offer. Java 26.1.2 encodes this as the
/// item registry id, a positive VarInt count, then a list of exact data
/// components. Solaris currently supports the empty exact-component predicate
/// only; non-empty predicates reject instead of silently accepting the wrong
/// item variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerchantItemCost {
    pub item_id: u32,
    pub count: i32,
}

impl MerchantItemCost {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.count <= 0 {
            return Err(CodecError::NotSupported(
                "merchant item cost count must be positive",
            ));
        }
        buf.write_varint(self.item_id as i32);
        buf.write_varint(self.count);
        // DataComponentExactPredicate.STREAM_CODEC / ByteBufCodecs.list().
        buf.write_varint(0);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let item_id = buf.read_varint()?;
        let item_id = u32::try_from(item_id)
            .map_err(|_| CodecError::NotSupported("negative merchant item id"))?;
        let count = buf.read_varint()?;
        if count <= 0 {
            return Err(CodecError::NotSupported(
                "merchant item cost count must be positive",
            ));
        }
        let components = read_count(buf, MAX_MERCHANT_COST_COMPONENTS)?;
        if components != 0 {
            return Err(CodecError::NotSupported(
                "merchant exact component predicates are unsupported",
            ));
        }
        Ok(Self { item_id, count })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MerchantOffer {
    pub cost_a: MerchantItemCost,
    pub result: ItemStack,
    pub cost_b: Option<MerchantItemCost>,
    pub out_of_stock: bool,
    pub uses: i32,
    pub max_uses: i32,
    pub xp: i32,
    pub special_price: i32,
    pub price_multiplier: f32,
    pub demand: i32,
}

impl MerchantOffer {
    fn validate(&self) -> Result<(), CodecError> {
        if self.result.is_empty() {
            return Err(CodecError::NotSupported(
                "merchant result must be non-empty",
            ));
        }
        if self.uses < 0 || self.max_uses <= 0 || self.uses > self.max_uses || self.xp < 0 {
            return Err(CodecError::NotSupported("invalid merchant offer counters"));
        }
        if !self.price_multiplier.is_finite() || self.price_multiplier < 0.0 {
            return Err(CodecError::NotSupported(
                "invalid merchant price multiplier",
            ));
        }
        Ok(())
    }

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.validate()?;
        self.cost_a.encode(buf)?;
        self.result.encode(buf)?;
        buf.write_bool(self.cost_b.is_some());
        if let Some(cost_b) = &self.cost_b {
            cost_b.encode(buf)?;
        }
        buf.write_bool(self.out_of_stock);
        buf.put_i32(self.uses);
        buf.put_i32(self.max_uses);
        buf.put_i32(self.xp);
        buf.put_i32(self.special_price);
        buf.put_f32(self.price_multiplier);
        buf.put_i32(self.demand);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let cost_a = MerchantItemCost::decode(buf)?;
        let result = ItemStack::decode(buf)?;
        let cost_b = buf
            .read_bool()?
            .then(|| MerchantItemCost::decode(buf))
            .transpose()?;
        let offer = Self {
            cost_a,
            result,
            cost_b,
            out_of_stock: buf.read_bool()?,
            uses: buf.read_i32()?,
            max_uses: buf.read_i32()?,
            xp: buf.read_i32()?,
            special_price: buf.read_i32()?,
            price_multiplier: buf.read_f32()?,
            demand: buf.read_i32()?,
        };
        offer.validate()?;
        Ok(offer)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundMerchantOffers {
    pub container_id: i32,
    pub offers: Vec<MerchantOffer>,
    pub villager_level: i32,
    pub villager_xp: i32,
    pub show_progress: bool,
    pub can_restock: bool,
}

impl Packet for ClientboundMerchantOffers {
    // `.analysis/protocol-dump.txt`: clientbound game registration index 52.
    const ID: i32 = 0x34;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.container_id < 0 || self.villager_level <= 0 || self.villager_xp < 0 {
            return Err(CodecError::NotSupported("invalid merchant header"));
        }
        if self.offers.len() > MAX_MERCHANT_OFFERS {
            return Err(CodecError::StringTooLong {
                len: self.offers.len(),
                max: MAX_MERCHANT_OFFERS,
            });
        }
        buf.write_varint(self.container_id);
        write_count(buf, self.offers.len())?;
        for offer in &self.offers {
            offer.encode(buf)?;
        }
        buf.write_varint(self.villager_level);
        buf.write_varint(self.villager_xp);
        buf.write_bool(self.show_progress);
        buf.write_bool(self.can_restock);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let container_id = buf.read_varint()?;
        if container_id < 0 {
            return Err(CodecError::NotSupported("invalid merchant container id"));
        }
        let offer_count = read_count(buf, MAX_MERCHANT_OFFERS)?;
        let mut offers = Vec::with_capacity(offer_count);
        for _ in 0..offer_count {
            offers.push(MerchantOffer::decode(buf)?);
        }
        let villager_level = buf.read_varint()?;
        let villager_xp = buf.read_varint()?;
        if villager_level <= 0 || villager_xp < 0 {
            return Err(CodecError::NotSupported("invalid merchant level or xp"));
        }
        Ok(Self {
            container_id,
            offers,
            villager_level,
            villager_xp,
            show_progress: buf.read_bool()?,
            can_restock: buf.read_bool()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundSelectTrade {
    pub offer_index: i32,
}

impl Packet for ServerboundSelectTrade {
    // `.analysis/protocol-dump.txt`: serverbound game registration index 51.
    const ID: i32 = 0x33;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.offer_index < 0 {
            return Err(CodecError::NotSupported("negative merchant offer index"));
        }
        buf.write_varint(self.offer_index);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let offer_index = buf.read_varint()?;
        if offer_index < 0 {
            return Err(CodecError::NotSupported("negative merchant offer index"));
        }
        Ok(Self { offer_index })
    }
}
