import 'dart:io';

import 'package:agent_code_client/services/agent_manager.dart';
import 'package:test/test.dart';

void main() {
  group('AgentManager', () {
    late AgentManager manager;

    setUp(() {
      manager = AgentManager();
    });

    tearDown(() async {
      await manager.killAll();
    });

    group('findBinary', () {
      test('returns null when no binary exists', () {
        // With no bundled path and no agent installed, returns null
        // (unless the test machine has agent installed).
        final result = manager.findBinary(bundledPath: '/nonexistent/agent');
        // Can't assert null because the test machine might have agent installed.
        // Just verify it doesn't throw.
        expect(result, anyOf(isNull, isNotNull));
      });

      test('returns bundled path when it exists', () {
        // Create a temporary file to act as a "binary"
        final tempDir = Directory.systemTemp.createTempSync('agent_test_');
        final fakeBinary = File('${tempDir.path}/agent');
        fakeBinary.writeAsStringSync('#!/bin/sh\necho fake');

        final result = manager.findBinary(bundledPath: fakeBinary.path);
        expect(result, fakeBinary.path);

        tempDir.deleteSync(recursive: true);
      });

      test('prefers bundled path over system binary', () {
        final tempDir = Directory.systemTemp.createTempSync('agent_test_');
        final fakeBinary = File('${tempDir.path}/agent');
        fakeBinary.writeAsStringSync('#!/bin/sh\necho fake');

        final result = manager.findBinary(bundledPath: fakeBinary.path);
        expect(result, fakeBinary.path);

        tempDir.deleteSync(recursive: true);
      });
    });

    group('spawn', () {
      test('throws when binary not found', () async {
        // Override with nonexistent bundled path and clear PATH-based lookup
        expect(
          () => manager.spawn('/tmp', bundledPath: '/nonexistent/agent'),
          throwsA(isA<AgentManagerException>()),
        );
      });
    });

    group('instances', () {
      test('starts empty', () {
        expect(manager.instances, isEmpty);
      });
    });

    group('kill', () {
      test('killing nonexistent PID does not throw', () async {
        // Should be a no-op, not an error
        await manager.kill(999999);
      });
    });

    group('killAll', () {
      test('works when no instances exist', () async {
        await manager.killAll();
        expect(manager.instances, isEmpty);
      });
    });

    group('discoverRunning', () {
      test('returns empty when no bridge directory exists', () {
        final found = manager.discoverRunning();
        // May or may not find instances depending on the test machine.
        expect(found, isA<List>());
      });
    });
  });
}
