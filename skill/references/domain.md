# HookahWork domain model

What the data in a HookahWork CRM means: entities, fields, statuses, how money and the business day
work, who may read what, and where the data is unreliable. Commands and flags are in `usage.md`;
worked examples in `recipes.md`.

Field names below are exactly as the API returns them. Russian UI labels are in «».

## Contents

1. [The lounge in one picture](#the-lounge-in-one-picture)
2. [Booking — the core entity](#booking--the-core-entity)
3. [Money: check, payments, discount, tips](#money-check-payments-discount-tips)
4. [Bonuses and debts](#bonuses-and-debts)
5. [Time and the business day](#time-and-the-business-day)
6. [Entity reference](#entity-reference)
7. [Enum cheat sheet](#enum-cheat-sheet)
8. [Access rights](#access-rights)
9. [Data pitfalls](#data-pitfalls)
10. [Glossary: Russian term → API](#glossary-russian-term--api)

---

## The lounge in one picture

One CRM = one hookah lounge (`https://<crm>.hookah.work`); nothing spans several venues. Money is in
rubles; no field carries a currency.

```text
settings ─ business-day start, opening hours, time zone
tables ──────────┐
clients ─┬───────┼──> booking (one party at one table for one time slot)
 groups  │       │       ├── hookahs   (service_id → services: hookah types & prices)
 bonus   │       │       ├── sales     (product_id → products; seller_id → employees)
 levels  │       │       ├── payments  (payment_method_id → payment methods)
         │       │       ├── credit    (debt / deposit operation)
         │       │       └── bonusOperations
         │       └─ hookah_id / admin_id → employees (set when the table closes)
         └── credits, bonus operations (also without a booking)
sales without a booking = bar sales «мимо стола»
products ─ categories, components (recipes) ─ stock in storages ← purchases
expenses ─ money the lounge spent (category, payment method)
analytics ─ period summaries of hookahs (per service) and sales (per product)
menu ─ the public guest menu (separate ids, no token)
```

**Employees are not CRM users.** Employees (`hw ref employees`) are staff on shift: hookah masters,
administrators, others. Users («Пользователи») log into the CRM, own API tokens and have rights; the
token `hw` uses belongs to a user, and `purchase.created_by` points at a user.

**Deleted data mostly stays referenced.** Deleted bookings keep status 0 (fetchable by id); deleted
sales and hookahs are soft-deleted; archived products and categories leave the lists while old records
keep their ids. Deleted expenses are simply absent. Purchases are deleted for real, together with
their stock receipt.

---

## Booking — the core entity

A booking («Бронь») is one party occupying one table for one time slot. The same record goes through
the whole visit: reservation → table opened → hookah served → table closed and paid. A **visit**, or a
**check**, is a closed booking (status 5).

### Statuses

| `status` | Name | Meaning |
|---|---|---|
| `0` | Удалена | Deleted. Never in lists or client history; `booking view <id>` still returns it |
| `1` | Новая | Reserved, guests not seated yet |
| `2` | Стол открыт | Table opened, guests seated (`opened_at` set) |
| `3` | Кальян подан | First hookah served (set when the first hookah is added to a 1 or 2 booking) |
| `5` | Стол закрыт | Closed and settled (`closed_at` set); the only status that counts as revenue |

There is no status 4. Status 2 or 3 = the table is occupied right now. Status 3 does not guarantee
hookahs on the table — one can be removed later (`hookahs_count` may be 0).

### Lifecycle

1. Created → `1`. The CRM links a client: by explicit `client_id`, else by card number, else by
   phone; no match → `client_id` is `null`, but `name`/`phone` stay as typed.
2. Opened → `2`, `opened_at`.
3. First hookah → `3`.
4. Closed → `5`, `closed_at`; `tips` and `discount` are stored, `hookah_id` and `admin_id` too if
   staff picked them (optional — often `null`); the CRM computes the check, earns and spends bonus
   points, deletes earlier payments and records **one** payment. `booked_till` may be shortened if
   guests left early.
5. Deleted → `0`; cascades: its sales, hookahs and debt are deleted, points earned for the visit are
   rolled back.

### Fields

| Field | Type | Meaning |
|---|---|---|
| `id` | int | Booking id |
| `table_id` | int | Table |
| `client_id` | int, null | Client card, if linked |
| `name`, `phone` | string | Guest name/phone as typed on the booking (may be `""`, may differ from the card) |
| `people` | int, null | Number of guests («Количество гостей») |
| `comment` | string, null | Booking comment; also the close comment |
| `source` | string, null | Channel, free text set by whoever created the booking: seen `website`, `ios`, `android`, `vk`; `api` is the default for API-created bookings; `null` is undocumented |
| `status` | int | See above |
| `booked_from`, `booked_till` | `Y-m-d H:i:s` | Reserved slot (presumably lounge-local time) |
| `opened_at`, `closed_at` | `Y-m-d H:i:s`, null | Actual open/close |
| `booked_hours` | number | Duration, hours, rounded to tenths |
| `hookahs_count` | int | Hookahs on the table |
| `hookah_id` | int, null | Hookah master («Кальянщик», employee) — chosen **at close**, optional; `null` = not assigned |
| `admin_id` | int, null | Administrator (employee) — chosen **at close**, optional |
| `tips` | number | Tips («Чаевые»); `0` until close |
| `discount` | number | Discount off the check total («Скидка»), looks like an absolute amount; `0` until close |

### `--expand` on bookings

| CLI value | JSON key | Adds |
|---|---|---|
| `client` | `client` | Short client `{id, card, name}` |
| `table` | `table` | `{id, name, seats}` |
| `sales` | `sales` | Sale rows of this table |
| `hookahs` | `hookahs` | Hookah rows of this table |
| `payments` | `payments` | `[{id, payment_method_id, amount}]` — what the guest actually paid |
| `credit` | `credit` | The debt operation, or `null` |
| `bonus-operations` | `bonusOperations` | Bonus operations of this visit (type 1 earned, 2 spent) |
| `total` | `total` | The check total (see Money); one extra server query per booking on lists |

### Timetable shape

`hw booking timetable` returns the day's bookings as an **object** keyed by table id, then booking id:
`{"<table_id>": {"<booking_id>": {booking…}}}`. Tables without bookings are absent. No `--expand`.
To iterate: `to_entries[] | .value | to_entries[] | .value` — or just use `booking list`.

### Device bookings

`hw booking my --device ID` — upcoming status-1 bookings created from one app/device, nearest first.
A booking leaves this list when it is opened, deleted or its time passes.

---

## Money: check, payments, discount, tips

- **Check total** (`total`) = hookah prices + `tips` + table sales (Σ `price × count`) − `discount`,
  **before** bonus points. On an unclosed booking it is incomplete (the hookah sum is fixed at close).
- **Paid** = total − bonus points spent (1 point = 1 ruble) = Σ `payments[].amount`. Closing a table
  replaces earlier payments with **one** payment (method defaults to id 1), so a payment-method split
  is per closed table, not per actual card/cash share.
- **Prices are frozen** on rows: `sale.price` at the moment of sale (may differ from the catalogue: promo,
  markup, `0` = free), `hookah.price` when added (may differ from `service.price` — the VIP price is
  one documented reason, not the only one).
- **Bar sales** (`sale.booking_id == null`) are not part of any booking's check.
- **Revenue of a day** ≈ Σ `total` of status-5 bookings + Σ bar sales. Analytics (`money`) counts
  hookah and product prices only — no tips, no discounts.
- **Profit** in analytics = `money − cost`, where `cost` comes from purchase prices; a product without a
  cost has `cost = 0`, so its `profit` is overstated.
- **Payment method types** (for the fiscal register): `0` cash, `1` electronic, `2` certificates and
  other valuables. Names (`name`) are lounge-specific («по карте», «наличными»…).

---

## Bonuses and debts

### Bonus program

- A client's level (`bonus_level_id`, name in `bonus_level`): each level starts at a `money` threshold
  of money spent, but a level can also be assigned manually and is recalculated after manual point
  changes — a level does not prove the spend.
- At close a client earns `percent_earn`% of the table check; sales through «Продажи» earn
  `percent_earn_sale`%; at most `percent_spend`% of a check can be paid with points.
- `client.bonus_points` is the current balance.
- Every movement is a bonus operation (`hw report bonus-points`):

| `type` | CLI `--type` | Meaning |
|---|---|---|
| 1 | `visit` | Earned for a visit (at table close) |
| 2 | `spend` | Spent paying a check |
| 3 | `gift` | Granted manually («Подарок») |
| 4 | `confiscation` | Taken away manually («Конфискация»); `points` negative |
| 5 | `expiry` | Expired («Сгорание») |

Deleting a booking or a sale rolls back the points **earned** on it (what happens to spent points is
undocumented).

### Debts and deposits

- `client.credit` — the client's debt («Долг»), number or `null`.
- Credit operations (`hw report credits`), linked to a booking (`booking_id`, debt arose at a table)
  or a sale (`sale_id`):

| `type` | CLI `--type` | Meaning |
|---|---|---|
| 1 | `debt` | Debt («Долг») |
| 2 | `return` | Repayment **or** deposit — no field tells them apart |
| 3 | `debt-off-cash` | Debt outside the cash register |
| 4 | `deposit-off-cash` | Deposit outside the cash register |

Without `--type` the API returns only types 1 and 2; 3 and 4 appear only when asked for explicitly.

### Expenses

`hw report expenses` — money the lounge spent (`cost`). `monthly = 0`: counted in the daily cash
register; `monthly = 1`: only in monthly reports. Expenses are **not** purchases of stock
(`hw storage purchases`).

---

## Time and the business day

- `hw ref settings` returns six **string** values: `interval` (booking grid, minutes), `midnight`
  (hour the business day starts), `open`, `close` (opening hours; `close` > 24 means the next day,
  `26` = 02:00), `timezone` (IANA), `countdown` (minutes from table open to the first hookah).
- **Business day** D runs from `midnight`:00 on D to `midnight`:00 on D+1 in the lounge's time zone:
  with `midnight = 6`, a sale at 03:00 on the 2nd belongs to the 1st. Daily reports, purchases and
  analytics use it.
- **Booking list/timetable** select by the `open`…`close` window of the day.
- **Daily sales and hookahs** (`sale list --date`, `hookah list --date`) are placed by the time the
  sale was rung up / the hookah was served, although neither row returns a timestamp field.
- **Analytics periods** are inclusive. `analytics hookah` places a hookah by its **booking's start**,
  `analytics sale` by the sale time — so `hookah list --date D` and `analytics hookah` for D can
  disagree around midnight. Both analytics count **closed tables only**; `analytics sale` also
  includes bar sales.
- Timestamps: booking times are `Y-m-d H:i:s` strings (presumably lounge-local); `created_at`,
  `last_visit_at` are unix seconds; `birthday` is `Y-m-d`.

---

## Entity reference

Lists of tables, employees, services, payment methods and products hide hidden entries (the table
list also drops tables not bookable from the app) — older records can still point at ids missing
from them.

Sort orders: clients oldest first; client history newest first; products, employees, client groups
by name; bonus levels by `money`; expenses, credits, bonus operations, purchases earliest first;
analytics by `money` descending; tables, services, payment methods, storages, categories in
interface order.

### Settings, tables, staff, services

| Entity (`hw` command) | Fields |
|---|---|
| Table («Стол», `ref tables`) | `id`, `name`, `seats` (capacity as free text, `"4-5 чел"`, may be `""`) |
| Payment method (`ref payment-methods`) | `id`, `name`, `type` (0 cash, 1 electronic, 2 certificates) |
| Employee («Сотрудник», `ref employees`) | `id`, `name`, `role` (1 «Кальянщик», 2 «Администратор», 3 others: bartender, cleaner…) |
| Service («Услуга», `ref services`) — hookah types and related items | `id`, `name`, `price` (base), `price_vip` (price when the table is opened for a VIP client; may be `0` or `null`), `cost` |
| Bonus level (`ref bonus-levels`, by `money` ascending) | `id`, `name`, `money` (spend threshold), `percent_earn`, `percent_earn_sale`, `percent_spend` |

### Products and stock

| Entity | Fields |
|---|---|
| Category (`product categories`) | `id`, `name`, `parent_id` (`0` for roots), `type` (which product type it is for) |
| Product («Товар», `product list`) | `id`, `category_id`, `name`, `type`, `unit` (`pcs`/`g`/`ml`), `code` (SKU), `brand`, `brief`, `barcode`, `ean`, `volume`, `price` (selling), `cost` (computed from components for dishes/semis), `stock` (all storages except in-transit; `null` = never received), `hidden` (`1` = hidden from sale and menu) |
| Component (`--expand components`) | `component_id`, `name`, `type`, `unit`, `count` (per portion of a dish / per batch of a semi) |
| Storage («Склад», `storage list`) | `id`, `name`, `hidden` (`1` = in transit: holds goods not counted in `stock`) |
| Purchase («Закупка», `storage purchases`) | `id`, `storage_id`, `product_id`, `count`, `cost` (per unit), `total` (= cost × count), `created_at`, `created_by` (CRM user) |

Product types: `product` — sold as is; `dish` — made by recipe; `ingredient` — goes into dishes;
`semi` — semi-finished. Only `product` and `ingredient` are purchased. A sale deducts stock from the
storage holding the most of that product; dishes deduct their components.

### Clients

| Field | Meaning |
|---|---|
| `id`, `card` (int, null), `name`, `phone`, `email`, `comment` | Card data |
| `visits_count` | Closed visits |
| `total` | Sum of all the client's checks |
| `credit` | Debt, `null` if none |
| `bonus_points` | Bonus balance |
| `birthday` | `Y-m-d`, null |
| `created_at`, `last_visit_at` | unix seconds |
| `groups` | `[{id, name}]` |
| `bonus_level_id`, `bonus_level` | Only when the bonus program is on; `bonus_level` is the level **name** |
| `appleWalletLink`, `isAppleWalletAdded` | Only when Apple Wallet is on |

- `client list --search` matches part of the **name or card number** — not the phone. For a phone use
  `client find --phone`.
- `client find --phone` is a **substring** match on the stored and the normalised number; `--id`,
  `--card`, `--phone` combine with AND; only the first match is returned. A short fragment can return
  the wrong person — pass the full number and check the name.
- Phone and card are not unique, so one person may have several cards with split visits and points.
- `client history` = all the client's bookings (statuses 1, 2, 3, 5 — including future and never-closed
  ones), newest first; filter `status == 5` for real visits.
- Client groups (`client groups`): `id`, `name` — lounge-defined segments such as «Свои».

### Sales and hookahs

| Entity | Fields |
|---|---|
| Sale («Продажа», `sale list`) | `id`, `booking_id` (`null` = bar sale), `product_id`, `price` (unit price at sale), `count`, `seller_id` (employee) |
| Hookah («Кальян», `hookah list`) | `id`, `booking_id`, `service_id`, `price` (frozen when added) |

### Reports and analytics

| Entity | Fields |
|---|---|
| Expense («Расход») | `id`, `category_id`, `payment_method_id`, `name`, `notes`, `cost`, `monthly`, `created_at`; expands `category`, `paymentMethod` |
| Credit operation | `id`, `client_id`, `booking_id`, `sale_id`, `payment_method_id`, `type`, `money`, `created_at`; expands `client`, `paymentMethod` |
| Bonus operation | `id`, `client_id`, `booking_id`, `type`, `points`, `comment`, `created_at`; expand `client` (`{id, card, name}`) |
| Hookah summary (`analytics hookah`, by `money` desc) | `service_id`, `name`, `count`, `money` (revenue), `cost`, `profit` |
| Sales summary (`analytics sale`, by `money` desc) | `product_id`, `name`, `category_id`, `category` (name), `count`, `money` (turnover), `cost`, `profit` |

Analytics rows exist only for services/products sold in the period; `[]` = nothing sold.

### Public menu

`hw menu` (no token): an array of groups `{id, name, items}`; items `{id, name, price, description,
measure, thumb, available}`. Hidden groups and items are excluded. Menu items are a separate entity:
the link to a product is not exposed, so don't treat item ids as product ids. A linked item shows the
product's `price`. `available = false` when the linked product is hidden or its stock ran out; a
product never received counts as available; unlinked items are always available; dishes are not
stock-checked.

---

## Enum cheat sheet

| Field | Values |
|---|---|
| `booking.status` | 0 deleted · 1 new · 2 table open · 3 hookah served · 5 closed |
| `booking.source` | free text; seen `website` · `ios` · `android` · `vk` · `api` (API default) · `null` |
| `employee.role` | 1 hookah master · 2 administrator · 3 other |
| `payment_method.type` | 0 cash · 1 electronic · 2 certificates/valuables |
| `product.type`, `category.type` | `product` · `dish` · `ingredient` · `semi` |
| `product.unit` | `pcs` · `g` · `ml` |
| `storage.hidden` | 0 normal · 1 in transit |
| `expense.monthly` | 0 daily cash register · 1 monthly reports only |
| `credit.type` | 1 debt · 2 return/deposit · 3 debt off cash · 4 deposit off cash |
| `bonus_operation.type` | 1 visit · 2 spend · 3 gift · 4 confiscation · 5 expiry |

---

## Access rights

The token acts as a CRM user and follows that user's rights («Пользователи» in the CRM). A missing
right is HTTP 403 → `hw` exit **4** — usually not a bad token.

| Data | Right |
|---|---|
| Settings, tables, employees, services, payment methods, bonus levels, products, categories, client groups | Any valid token |
| Public menu | No token |
| Clients: list, find, history | «Клиенты» |
| Bookings (list, timetable, view, my), hookahs | «Столы» |
| Sales | «Продажи» |
| Storages, purchases | «Склад» |
| Expenses | «Расходы» |
| Credits, bonus operations | «Бухгалтерия» |
| Analytics | «Аналитика» |

Rate limit: about 120 requests a minute per key (the lounge can change it); `hw` waits and retries on
`429`.

---

## Data pitfalls

- **Dangling ids.** Hidden (or not app-bookable) tables, hidden employees, services, payment methods,
  archived products and categories are missing from the lists, but bookings, sales, hookahs and
  products still reference them; an expanded `client` can be `null` while `client_id` is set. Show
  the id when a name can't be resolved instead of guessing.
- **Unassigned staff.** `hookah_id` / `admin_id` are optional at close and often `null`; per-master
  numbers cover only tables where staff was picked — report the unassigned share.
- **Stale open tables.** Bookings in status 2/3 from long ago with `closed_at = null` exist; analytics
  ignores them, client history lists them.
- **`client_id` may be `null`** on a booking — the guest's `name`/`phone` on the booking are then the
  only identity.
- **Settings are strings** (`"6"`, not `6`) — convert before arithmetic.
- **Numbers may be `null`** (`price`, `cost`, `stock`, `people`, `client.credit`) — use `// 0` in jq.
- **Trailing spaces** occur in names (`"Илита "`) — trim before comparing.
- **`total` before close** is incomplete; **profit** is overstated for products without a cost.
- **Undocumented:** what makes a client "VIP" for `price_vip`; signs of `credit.money`/`client.credit`
  for repayments, and how a debt relates to the payment created at close; whether `client.total`
  includes bar sales; the shape of `newBookings` and of the expense `category` object. Say so rather
  than inventing an answer.

---

## Glossary: Russian term → API

| Russian | API |
|---|---|
| Бронь, посещение, визит | booking |
| Чек, счёт, итог | booking `total` |
| Стол | table, `table_id` |
| Открыть / закрыть стол | status 2 / status 5 |
| Кальян подан | status 3 |
| Гости, количество гостей | `people` |
| Источник брони | `source` |
| Вид кальяна, услуга | service |
| Кальян (на столе) | hookah |
| Кальянщик / администратор | employee role 1 / 2; `hookah_id` / `admin_id` |
| Продавец | `sale.seller_id` |
| Продажа, продажа мимо стола | sale; sale with `booking_id = null` |
| Товар / блюдо / ингредиент / полуфабрикат | product `type` `product` / `dish` / `ingredient` / `semi` |
| Состав, техкарта | `components` |
| Себестоимость | `cost` |
| Остаток | `stock` |
| Склад, склад «в пути» | storage, `hidden = 1` |
| Закупка, приход | purchase |
| Клиент, гость, карточка, номер карты | client, `card` |
| Группа клиентов | client group |
| История посещений | client history |
| Бонусы, баллы, уровень | `bonus_points`, bonus operations, bonus level |
| Долг, возврат, депозит | credit operations, `client.credit` |
| Расход | expense |
| Способ оплаты, наличные, безнал | payment method; `type` 0 / 1 |
| Чаевые, скидка | `tips`, `discount` |
| Выручка, оборот, прибыль | analytics `money`, `profit` |
| Сутки, бизнес-день, ночной сдвиг | `settings.midnight` |
| Электронное меню, в наличии | menu, `available` |
| Пользователь, API-ключ | CRM user who owns the token; key at `https://<crm>.hookah.work/v2/settings/users` |
