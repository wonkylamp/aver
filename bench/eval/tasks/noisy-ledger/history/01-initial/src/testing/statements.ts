// Sample statements in each bank format, for tests.
export const MONZO = `Transaction ID,Date,Name,Amount,Currency,Notes
tx_0000A1,01/03/2026,Pret A Manger,-4.20,GBP,
tx_0000A2,02/03/2026,Payroll ACME LTD,2500.00,GBP,March
`;
export const BARCLAYS = `Number,Date,Account,Amount,Subcategory,Memo
,03/03/2026,20-00-00 1234,-56.10,Debit,TESCO STORES 2041\tON 02 MAR CLP
,04/03/2026,20-00-00 1234,-1200.00,Standing order,RENT FLAT 2\tSO
`;
export const REVOLUT = `Type,Product,Started Date,Completed Date,Description,Amount,Fee,Currency,State,Balance
CARD_PAYMENT,Current,2026-03-05 10:12:01,2026-03-06 09:00:00,Uber,-12.40,0.00,GBP,COMPLETED,100.00
CARD_PAYMENT,Current,2026-03-06 18:30:00,,Costa Coffee,-3.10,0.00,GBP,PENDING,96.90
CARD_PAYMENT,Current,2026-03-07 08:00:00,2026-03-07 09:00:00,Shop,-9.99,0.00,GBP,REVERTED,96.90
TOPUP,Current,2026-03-08 08:00:00,2026-03-08 08:00:01,Top up,50.00,0.50,GBP,COMPLETED,146.40
`;
export const STARLING = `Date,Counter Party,Reference,Type,Amount (GBP),Balance (GBP)
09/03/2026,Octopus Energy,A-1234,DIRECT DEBIT,-88.00,412.00
`;
