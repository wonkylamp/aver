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
