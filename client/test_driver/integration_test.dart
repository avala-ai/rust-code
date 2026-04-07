// This file is the entry point for `flutter drive` on web.
// It bootstraps the integration test adapter that bridges between
// the `integration_test` package and the `flutter_driver` protocol.

import 'package:integration_test/integration_test_driver.dart';

Future<void> main() => integrationDriver();
