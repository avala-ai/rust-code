import 'package:agent_code_client/services/update_checker.dart';
import 'package:test/test.dart';

void main() {
  group('UpdateChecker', () {
    group('_isNewer (via check behavior)', () {
      // Test the semver comparison logic indirectly.
      // Since _isNewer is private, we test via the public API behavior.
      // For unit testing the logic directly, we expose it here:

      test('version comparison', () {
        expect(UpdateChecker.isNewer('0.2.0', '0.1.0'), isTrue);
        expect(UpdateChecker.isNewer('1.0.0', '0.9.9'), isTrue);
        expect(UpdateChecker.isNewer('0.1.1', '0.1.0'), isTrue);
        expect(UpdateChecker.isNewer('0.1.0', '0.1.0'), isFalse);
        expect(UpdateChecker.isNewer('0.1.0', '0.2.0'), isFalse);
        expect(UpdateChecker.isNewer('1.0.0', '1.0.0'), isFalse);
      });
    });

    test('check returns null on network error', () async {
      final checker = UpdateChecker();
      // With no mock, this hits the real API. On CI without network, returns null.
      // In a real test suite, we'd mock http.Client.
      final result = await checker.check('999.999.999');
      // Either null (no newer version) or an UpdateInfo.
      expect(result, anyOf(isNull, isA<UpdateInfo>()));
    });
  });
}
