# Shipping

How the parcel desk prices, labels and hands over parcels.

## Addresses

An address has a name, one to three lines, a town, a postcode and an ISO
country code (`GB` for the United Kingdom). Postcodes are printed and looked
up in upper case.

## Zones

The carrier's zone of a domestic address comes from its postcode area and
district (`KW` and `15` of `KW15 1AA`), from the carrier's "Zones and
surcharge areas" sheet: Northern Ireland (`BT`), islands, Highlands, and the
mainland for everything else. Addresses abroad are `international`.

## Services

| Service | Transit (business days) | Volumetric divisor (cm³/kg) |
|---|---|---|
| economy | 3 | 6000 |
| express | 1 | 5000 |

A parcel's delivery day is its transit time in business days (Monday to
Friday) after the day it is handed over.

## Prices

A quote is the rate card's band price for the parcel's weight, service and
zone, then the surcharges in this order:

1. `FUEL`: 3% of the band price, rounded half up.

The net price is the band price plus the surcharges; VAT is 20% of the net
price, rounded half up; the total is net plus VAT. All amounts are integer
pence.

Parcels weigh at most 30 kg, are at most 150 cm long and have a girth
(length + 2 × width + 2 × height) of at most 300 cm.

## Tracking numbers

UPU S10: two letters (`JD`), an eight-digit serial, a check digit, `GB`.
The check digit weights the serial's digits by 8 6 4 2 3 5 9 7 and is 11
minus the sum modulo 11, where 10 becomes 0 and 11 becomes 5.

## Labels

Forty characters a line; longer lines are cut with `…`. In order:

1. `DISPATCH <SERVICE>`
2. a blank line
3. `TO: <name>`, the address lines, the town in capitals, the postcode
   (abroad: the postcode and the country code)
4. a blank line
5. `FROM: <name>, <postcode>`
6. `WEIGHT: <kg> kg`, to one decimal place
7. `TRACKING: <number>`

## Tracking

The carrier's feed has one event per line: an ISO 8601 time, a two-letter
code and an optional location. CR label created, CL collected, IT in
transit, OD out for delivery, DL delivered, EX delivery problem. Unknown
codes are ignored.

## Manifest

One CSV row per parcel under a header row: tracking, service, name, address
(the lines joined with ` / `), town, postcode, country, weight_kg (actual
weight to one decimal place).

## Planned for the spring release

Ten tickets, built at the same time. Where two of them touch the same thing,
this section says how they fit together; build yours so that it does.

### #41 Postcodes

`normalizePostcode` removes every space, upper-cases, and puts one space
before the last three characters: `sw1a1aa` and ` ec1a  1bb ` become
`SW1A 1AA` and `EC1A 1BB`. A string of fewer than five characters (without
spaces) is only upper-cased, with its spaces removed. `isValidPostcode`
accepts the UK formats only: `A9 9AA`, `A99 9AA`, `A9A 9AA`, `AA9 9AA`,
`AA99 9AA`, `AA9A 9AA` (A a letter, 9 a digit), and `GIR 0AA`. Zones,
labels and the manifest use the normalized postcode, so `kw151dh` is in
Orkney (islands) like `KW15 1DH`.

### #42 Check digits

Tracking numbers follow S10 as written above; today serials whose sum
leaves 0 or 1 get a two-digit "check digit".

### #43 Bank holidays

Business days are Monday to Friday except the bank holidays in
`src/holidays.ts`. `addBusinessDays(day, n, holidays = BANK_HOLIDAYS)`
skips them, so delivery estimates do: express handed over on Thursday 2
April 2026 arrives on Tuesday 7 April (Good Friday and Easter Monday are
holidays).

### #44 CSV quoting

A field that contains a comma, a double quote, a carriage return or a line
feed is written in double quotes, with each double quote doubled
(`Flat 2, 10 High St` is written `"Flat 2, 10 High St"`, `5" box` is
written `"5"" box"`). Other fields are written as they are. `parseCsv`
reads quoted fields back, line breaks inside them included. Rows still end
with `\n`.

### #45 Returns in tracking

Codes RS (returning to sender) and RD (returned to sender) are the statuses
`returning` ("Returning to sender") and `returned` ("Returned to sender").
The carrier's feed is not in time order: a parcel's status is that of its
latest event by time (times are compared as instants, offsets included;
of two events at the same instant the later in the feed wins), and the
timeline lists events oldest first.

### #46 Volumetric weight

Light, bulky parcels are priced by size. `volumetricKg(parcel, service)` in
`src/parcel.ts` is length × width × height in cm divided by the service's
volumetric divisor (above), and `chargeableKg(parcel, service)` is the
greater of the actual and the volumetric weight, rounded up to the next
0.1 kg. Quotes use the chargeable weight for the band and report it as
`chargeableKg`. The 30 kg limit stays on the actual weight; a chargeable
weight over the last band is a `RateError` as now.

### #47 Remote areas

A `REMOTE` surcharge, listed after `FUEL`: £4.50 in the Highlands; £9.00 on
the islands plus 50p per kg of chargeable weight over 10 kg (a part kg
counts as a whole kg); nothing elsewhere, and then no `REMOTE` entry.
`remoteSurcharge(zone, kg)` in `src/zones.ts` gives the amount in pence.
The fuel surcharge stays 3% of the band price alone; VAT is on the net price
as before.

### #48 Hazmat labels

A parcel with `hazmat` gets the line `UN3481 LITHIUM ION BATTERIES` right
after the `DISPATCH` line. Express does not carry them: the label of a
hazmat parcel by express is a `LabelError`.

### #49 Customs declarations

The label of a parcel going abroad ends with a CN22 block after the
`TRACKING` line: a blank line, `CN22 CUSTOMS DECLARATION`, one line per
contents item (`<qty> x <description> <HS code or -> <qty × value>`, for
example `2 x T-shirt 610910 £30.00`), and `TOTAL <sum of the lines>`. A
parcel going abroad without contents is a `LabelError`.

### #50 Chargeable weight in the manifest

Depends on #46. The carrier bills on chargeable weight, so the manifest gets
a `chargeable_kg` column right after `weight_kg`: `chargeableKg(parcel,
service)` from #46, to one decimal place. `weight_kg` stays the actual
weight.
