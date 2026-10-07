# Real protocol fixtures

These are **real mainnet bytes**, captured 2026-10-07 via `rpc.solami.dev`.
They contain no credentials. They make the decoder tests deterministic and
network-independent while still being grounded in real on-chain data.

| File | What |
|---|---|
| `pumpfun_bonding_curve.hex` | A real `BondingCurve` account (125-byte current layout) |
| `pumpfun_trade_event.hex` | The real `TradeEvent` emitted for a trade on that curve |
| `pumpfun_pair.txt` | The `mint` and `bonding_curve` for the fixture above |
| `pumpswap_pool.hex` | A real `pump_amm` `Pool` account |
| `pumpswap_buy_event.hex` | A real `pump_amm` `BuyEvent` |
| `pumpswap_sell_event.hex` | A real `pump_amm` `SellEvent` |

The bonding-curve account and its trade event are from the same market, so the
reserves decoded from each must agree — that cross-check is asserted in
`tests/real_fixtures.rs`.
