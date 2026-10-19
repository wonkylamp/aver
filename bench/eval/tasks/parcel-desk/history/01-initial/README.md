# dispatch

The parcel desk of the warehouse: prices a parcel for a service and a
destination, prints its label, reads the carrier's tracking feed and writes
the daily manifest the carrier collects with the parcels.

- `src/money.ts`: pence, rounding, formatting.
- `src/dates.ts`, `src/holidays.ts`: calendar days and business days.
- `src/address.ts`: addresses and UK postcodes.
- `src/zones.ts`: the carrier's delivery zone of an address.
- `src/parcel.ts`: parcels, their checks and measurements.
- `src/services.ts`: the carrier's services (economy, express).
- `src/rates.ts`: the rate card, weight bands per service and zone.
- `src/quote.ts`: the price of sending a parcel.
- `src/ids.ts`: tracking numbers.
- `src/labels.ts`: the text of the label printer.
- `src/tracking.ts`: the carrier's tracking events.
- `src/csv.ts`, `src/manifest.ts`: the manifest file.

Run the tests with `npm test` (Node 22.18 or later runs the TypeScript
sources directly). There are no dependencies.

docs/shipping.md is the specification: how prices, labels and the manifest
are made, and what is planned.
