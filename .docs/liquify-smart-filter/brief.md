# Liquify smart filters from PSD render from their stored mesh

**What:** a Photoshop Liquify smart filter (`filterFX` item, `Fltr` class `LqFy`) imports as
`psd.unsupportedFilter` and renders as a pass-through, so a smart object bent with Liquify
re-renders without the bend (and a headless render server refuses it).

**Why:** Photoshop stores the Liquify deformation in the filter (`LqMe`, raw bytes), the way it
stores Puppet Warp's solved mesh. Decoding it lets the smart object re-render (after Replace
Contents) as Photoshop draws it, with no Photoshop-side solve.

**Done when:** the `LqMe` mesh decodes into a displacement field, renders through the usual smart
filter pipeline (blend, opacity, visibility, filter mask), round-trips to PSD byte for byte, and a
re-render of a real Photoshop mockup's Liquify smart objects matches the pixels Photoshop stored
for those layers (mean |diff| well under 1/255). Data that doesn't decode stays unsupported.
