# Recipes: lounge questions → `hw` commands

Worked answers to the questions owners and managers actually ask. Field meanings are in `domain.md`,
flags in `usage.md`. All recipes are read-only.

Conventions used below:

- `-F toon` when you read the result; `-F json` / `-F csv` when a script or a file consumes it.
- `--jq` is jaq (a jq clone) and runs inside `hw` — no external `jq` needed. Several results come
  back as one array.
- Day loops use `seq -f '2026-09-%02g' 1 30` (works on macOS and Linux) and stay well under the rate
  limit; explicit dates on `booking list` cost no extra settings request.
- Before a new aggregation, look at one record: `hw -F json <command> --jq '.[0]'`.

## Contents

1. [Right now](#right-now)
2. [One day's results](#one-days-results)
3. [Periods: revenue, average check, top items](#periods-revenue-average-check-top-items)
4. [Staff](#staff)
5. [Clients](#clients)
6. [Money: expenses, debts, bonuses](#money-expenses-debts-bonuses)
7. [Stock and menu](#stock-and-menu)
8. [Exports](#exports)

---

## Right now

**Who is sitting where, and since when?** Status 2 = table open, 3 = hookah served.

```bash
hw -F toon booking list --expand client,table \
  --jq '[.[] | select(.status == 2 or .status == 3)
         | {table: .table.name, guest: (.client.name // .name), people, opened_at, hookahs_count}]'
```

**Who is still expected today?** Status 1, by start time.

```bash
hw -F toon booking list --expand table \
  --jq '[.[] | select(.status == 1) | {from: .booked_from[11:16], table: .table.name, name, phone, people}]
        | sort_by(.from)'
```

**Which tables are busy when tonight?**

```bash
hw -F toon booking list --date today --expand table \
  --jq 'group_by(.table_id)
        | map({table: .[0].table.name, slots: map("\(.booked_from[11:16])–\(.booked_till[11:16])")})'
hw -F toon ref tables        # tables missing above have no bookings today
```

**How many hookahs so far tonight?** Analytics only sees closed tables — count served hookahs instead.

```bash
hw -F json hookah list --date today --expand service \
  --jq 'group_by(.service_id) | map({service: .[0].service.name, count: length, sum: (map(.price) | add)})'
```

## One day's results

**Checks and revenue of a closed business day.** Closed tables (status 5) with their totals, plus bar
sales that belong to no table:

```bash
D=2026-10-01
hw -F json booking list --date "$D" --expand total \
  --jq '[.[] | select(.status == 5) | .total // 0]
        | {checks: length, tables_revenue: (add // 0), avg_check: ((add // 0) / ([length, 1] | max))}'
hw -F json sale list --date "$D" \
  --jq '[.[] | select(.booking_id == null) | .price * .count] | add // 0'     # bar sales
```

The check total includes tips and subtracts the discount; bonus points are not subtracted (see
"Money" in `domain.md`).

**How did guests pay?** Payments of closed tables by method. Closing a table leaves one payment
with one method, so a split bill shows up under a single method:

```bash
hw -F json booking list --date yesterday --expand payments \
  --jq '[.[] | select(.status == 5) | .payments[]]
        | group_by(.payment_method_id)
        | map({payment_method_id: .[0].payment_method_id, amount: (map(.amount) | add)})'
hw -F toon ref payment-methods
```

**Where did bookings come from?**

```bash
hw -F json booking list --date yesterday \
  --jq 'group_by(.source) | map({source: (.[0].source // "unknown"), bookings: length})'
```

**What was sold (bar and tables)?**

```bash
hw -F json sale list --date yesterday --expand product \
  --jq 'group_by(.product_id)
        | map({product: .[0].product.name, count: (map(.count) | add), sum: (map(.price * .count) | add)})
        | sort_by(-.sum)'
```

**Details of one table's bill:**

```bash
hw -F toon booking view 715 --expand client,table,hookahs,sales,payments,bonus-operations,total
```

## Periods: revenue, average check, top items

**Hookah types and products over a period** — one call each, inclusive dates:

```bash
hw -F toon analytics hookah --from 2026-09-01 --till 2026-09-30            # per service, by revenue
hw -F toon analytics hookah --from 2026-09-01 --till 2026-09-30 --jq 'sort_by(-.count)'   # by popularity
hw -F toon analytics sale --from 2026-09-01 --till 2026-09-30 --jq '.[:10]'               # top 10 products
```

**Turnover by product category:**

```bash
hw -F json analytics sale --from 2026-09-01 --till 2026-09-30 \
  --jq 'group_by(.category)
        | map({category: (.[0].category // "—"), money: (map(.money) | add), profit: (map(.profit) | add)})
        | sort_by(-.money)'
```

Analytics `money` is the sum of item prices: no tips, no discounts. `profit` is overstated for items
without a cost.

**Average check over a period** — one `booking list` per business day:

```bash
for d in $(seq -f '2026-09-%02g' 1 30); do
  hw -F csv booking list --date "$d" --expand total \
    --jq '[.[] | select(.status == 5) | {total: (.total // 0)}]' | tail -n +2
done | awk '{n++; s += $1} END {printf "checks=%d revenue=%.0f avg_check=%.0f\n", n, s, (n ? s / n : 0)}'
```

State the definition you used ("closed tables, check total before bonus points").

**Busiest days of the period** — closed tables per day:

```bash
for d in $(seq -f '2026-09-%02g' 1 30); do
  printf '%s ' "$d"
  hw -F json booking list --date "$d" --jq '[.[] | select(.status == 5)] | length'
done | sort -k2 -n -r | head
```

## Staff

**Tables and hookahs per hookah master on a day.** The master is picked at close and is optional —
the `hookah_id: null` group is "not assigned"; report it rather than dropping it:

```bash
hw -F json booking list --date yesterday \
  --jq '[.[] | select(.status == 5)] | group_by(.hookah_id)
        | map({hookah_id: .[0].hookah_id, tables: length, hookahs: (map(.hookahs_count) | add)})'
hw -F toon ref employees        # role 1 = hookah master, 2 = administrator
```

**Sales per seller:**

```bash
hw -F json sale list --date yesterday --expand seller \
  --jq 'group_by(.seller_id) | map({seller_id: .[0].seller_id, sum: (map(.price * .count) | add)})'
```

For a period, run the per-day command in a `seq` loop as above.

## Clients

Client data is personal: fetch what the question needs and don't copy it into other places unasked.

**Look a guest up and see their status:**

```bash
hw -F toon client find --phone 79175555111 --expand bonus-level,new-bookings   # full number: it is a substring match
hw -F toon client history 42 --per-page 5                        # latest five bookings
hw -F json client history 42 --all --jq '[.[] | select(.status == 5)] | length'   # real visits
```

**Best clients by money spent:**

```bash
hw -F json client list --all \
  --jq 'sort_by(-(.total // 0)) | .[:20] | map({id, name, visits_count, total, bonus_points})'
```

**Birthdays in October:**

```bash
hw -F json client list --all \
  --jq '[.[] | select(.birthday != null and .birthday[5:7] == "10") | {name, phone, birthday}]
        | sort_by(.birthday[8:10])'
```

**Regulars who stopped coming** (3+ visits, none in 60 days; `last_visit_at` is unix seconds):

```bash
cutoff=$(( $(date +%s) - 60 * 86400 ))
hw -F json client list --all \
  --jq "[.[] | select(.visits_count >= 3 and (.last_visit_at // 0) < $cutoff)
         | {id, name, phone, visits_count, total}]"
```

**Clients of a group** (find the group id first):

```bash
hw -F toon client groups
hw -F toon client list --group 3 --all
```

## Money: expenses, debts, bonuses

**Expenses of a day and of a month:**

```bash
hw -F toon report expenses --date yesterday --expand category,payment-method
for d in $(seq -f '2026-09-%02g' 1 30); do
  hw -F csv report expenses --date "$d" --jq '[.[] | {cost, monthly}]' | tail -n +2
done | awk -F, '{s += $1; if ($2 == 1) m += $1} END {printf "total=%.0f monthly_only=%.0f\n", s, m}'
```

`[]` means nothing was recorded that business day — not an error. Stock purchases are a different
thing: `hw storage purchases`.

**Who owes money:**

```bash
hw -F json client list --all \
  --jq '[.[] | select((.credit // 0) != 0) | {id, name, phone, credit}] | sort_by(-.credit)'
hw -F toon report credits --date yesterday --expand client        # debts and returns of a day
hw -F toon report credits --date yesterday --type debt-off-cash   # off-register debts (hidden by default)
```

**Bonus points earned and spent on a day:**

```bash
hw -F json report bonus-points --date yesterday \
  --jq 'group_by(.type) | map({type: .[0].type, operations: length, points: (map(.points) | add)})'
```

Types: 1 visit, 2 spend, 3 gift, 4 confiscation, 5 expiry.

## Stock and menu

**Out of stock:** (`stock = null` means never received — not the same as zero)

```bash
hw -F json product list --all \
  --jq '[.[] | select(.stock != null and .stock <= 0 and .hidden != 1) | {id, name, type, stock, unit}]'
```

**What came in today:**

```bash
hw -F json storage purchases --date today --expand product,storage \
  --jq '{total: (map(.total) | add // 0),
         items: map({product: .product.name, storage: .storage.name, count, cost, total})}'
```

**Recipe of a dish:**

```bash
hw -F toon product list --search "Мохито" --expand components
```

**Menu items guests can't order right now:**

```bash
hw -F json menu --jq '[.[] | .name as $group | .items[] | select(.available | not) | {group: $group, name}]'
```

## Exports

```bash
hw -F csv analytics sale --from 2026-09-01 --till 2026-09-30 > sales-2026-09.csv
hw -F csv client list --all > clients.csv                      # personal data — only on request
hw -F csv booking list --date yesterday --expand table \
  --jq 'map({id, table: .table.name, name, people, status, booked_from, closed_at})' > bookings.csv
```

CSV takes the union of keys as the header; nested objects become JSON cells — flatten them with
`--jq` first, as in the last example.
