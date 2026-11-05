# Test IDs

Stable `testID`s for Maestro and Playwright. Rename only together with the flows that use them. `N` is a zero-based index.

## Home `/`

| testID                 | Element                              |
| ---------------------- | ------------------------------------ |
| `home-screen`          | Screen                               |
| `home-scan-button`     | Opens `/scan`                        |
| `home-printer-input`   | Manual printer id field              |
| `home-printer-go`      | Opens `/p/{id}`                      |
| `home-printer-error`   | Invalid id message (only when shown) |
| `home-language-toggle` | Switches vi / en                     |

## Scan `/scan`

| testID                  | Element                                 |
| ----------------------- | --------------------------------------- |
| `scan-screen`           | Screen                                  |
| `scan-camera`           | Camera view (granted)                   |
| `scan-error`            | Rejected QR message (only when shown)   |
| `scan-again`            | Dismiss the rejection and scan again    |
| `scan-permission-grant` | Ask for camera permission (not granted) |
| `scan-manual`           | Back to home for manual entry           |

## Printer `/p/[printerId]`

| testID                | Element                                                    |
| --------------------- | ---------------------------------------------------------- |
| `printer-screen`      | Screen                                                     |
| `printer-loading`     | Spinner                                                    |
| `printer-name`        | Printer name                                               |
| `printer-price`       | Price per print, VND formatted                             |
| `printer-status`      | Status text; `busy` is an info note (still bookable)       |
| `printer-pick-button` | Start picking photos (only when online)                    |
| `printer-retry`       | Re-check the printer (`offline`/`error`) or retry on error |
| `printer-error`       | Load error box                                             |
| `printer-not-found`   | Unknown printer message                                    |
| `printer-home`        | Back to home from an error                                 |

## Photos `/p/[printerId]/photos`

| testID                | Element                               |
| --------------------- | ------------------------------------- |
| `picker-screen`       | Screen                                |
| `picker-loading`      | Spinner while the price loads         |
| `picker-error`        | Load error box (`picker-error-retry`) |
| `picker-add`          | Open the photo library                |
| `picker-empty`        | Empty hint                            |
| `picker-notice-N`     | Skipped-photo / limit notice          |
| `picker-photo-N`      | Photo row                             |
| `picker-copies-N`     | "Copies: n" label                     |
| `picker-copies-inc-N` | One more copy                         |
| `picker-copies-dec-N` | One fewer copy                        |
| `picker-remove-N`     | Remove the photo                      |
| `picker-total`        | Live total, VND formatted             |
| `picker-continue`     | Upload and pay                        |

## Upload `/p/[printerId]/upload`

| testID                  | Element                             |
| ----------------------- | ----------------------------------- |
| `upload-screen`         | Screen                              |
| `upload-phase`          | Header text for the current phase   |
| `upload-busy`           | Spinner (creating / checking out)   |
| `upload-photo-N`        | Photo row                           |
| `upload-photo-status-N` | Pending / progress / done / failed  |
| `upload-photo-retry-N`  | Retry this photo (only when failed) |
| `upload-error`          | Order or checkout error box         |
| `upload-retry`          | Retry the failed step               |
| `upload-back`           | Back (no photos selected)           |

## Order `/orders/[orderId]`

| testID               | Element                                                                          |
| -------------------- | -------------------------------------------------------------------------------- |
| `order-screen`       | Screen                                                                           |
| `order-loading`      | Spinner before the first response                                                |
| `order-error`        | Fetch error box                                                                  |
| `order-reference`    | "Order: {id}"                                                                    |
| `order-status`       | Current status text                                                              |
| `order-amount`       | Amount, VND formatted                                                            |
| `order-timeline`     | Timeline list                                                                    |
| `order-step-STATUS`  | One step: `awaiting_upload`, `awaiting_payment`, `paid`, `printing`, `completed` |
| `order-cancel`       | Cancel (only before paid)                                                        |
| `order-cancel-error` | Cancel failure box                                                               |
| `order-done`         | Completed result                                                                 |
| `order-failed`       | Failed result                                                                    |
| `order-cancelled`    | Cancelled result                                                                 |
| `order-expired`      | Expired result                                                                   |
| `order-again`        | Print more at the same printer (terminal states)                                 |
| `order-home`         | Back to home                                                                     |
