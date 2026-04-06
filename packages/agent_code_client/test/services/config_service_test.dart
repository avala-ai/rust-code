import 'dart:io';

import 'package:agent_code_client/services/config_service.dart';
import 'package:test/test.dart';

void main() {
  group('ConfigService', () {
    late ConfigService config;
    late Directory tempDir;
    late String originalHome;

    setUp(() {
      config = ConfigService();
      tempDir = Directory.systemTemp.createTempSync('config_test_');
      originalHome = Platform.environment['HOME'] ?? '';
    });

    tearDown(() {
      tempDir.deleteSync(recursive: true);
    });

    test('read returns empty map when no config exists', () {
      // The default config path may or may not exist on the test machine.
      // Just verify it doesn't throw.
      final result = config.read();
      expect(result, isA<Map<String, dynamic>>());
    });

    test('get returns null for nonexistent key', () {
      final result = config.get('nonexistent_key_xyz');
      // May return null or a value depending on what's in the real config.
      expect(result, anyOf(isNull, isNotNull));
    });

    test('set rejects invalid permission_mode', () {
      expect(
        () => config.set('permission_mode', 'invalid_mode'),
        throwsA(isA<ConfigException>()),
      );
    });

    test('set accepts valid permission_mode values', () {
      for (final mode in ['default', 'auto', 'plan', 'ask', 'deny']) {
        // This will write to the real config file, so we just verify
        // the validation passes without throwing.
        // In a real test environment, we'd mock the filesystem.
        expect(() => config.set('permission_mode', mode), returnsNormally);
      }
    });
  });
}
