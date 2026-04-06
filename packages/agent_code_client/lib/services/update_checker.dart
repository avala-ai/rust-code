import 'dart:convert';

import 'package:http/http.dart' as http;

/// Checks GitHub Releases API for newer versions of the desktop app.
class UpdateChecker {
  static const _releaseUrl =
      'https://api.github.com/repos/avala-ai/agent-code/releases';
  static const _desktopPrefix = 'desktop-v';

  /// Check if a newer desktop release exists.
  ///
  /// Returns the download URL and version if an update is available,
  /// or null if the current version is up to date.
  Future<UpdateInfo?> check(String currentVersion) async {
    try {
      final client = http.Client();
      try {
        final resp = await client
            .get(
              Uri.parse(_releaseUrl),
              headers: {'Accept': 'application/vnd.github.v3+json'},
            )
            .timeout(const Duration(seconds: 5));

        if (resp.statusCode != 200) return null;

        final releases = jsonDecode(resp.body) as List<dynamic>;

        for (final release in releases) {
          final tag = release['tag_name'] as String? ?? '';
          if (!tag.startsWith(_desktopPrefix)) continue;

          final version = tag.substring(_desktopPrefix.length);
          if (isNewer(version, currentVersion)) {
            // Find the DMG asset.
            final assets = release['assets'] as List<dynamic>? ?? [];
            final dmg = assets.firstWhere(
              (a) => (a['name'] as String).endsWith('.dmg'),
              orElse: () => null,
            );

            return UpdateInfo(
              version: version,
              downloadUrl: dmg != null
                  ? dmg['browser_download_url'] as String
                  : release['html_url'] as String,
              releaseNotes: release['body'] as String? ?? '',
            );
          }
        }
      } finally {
        client.close();
      }
    } catch (_) {
      // Network error, timeout, parse error. Silent failure.
    }
    return null;
  }

  /// Simple semver comparison: a > b?
  static bool isNewer(String a, String b) {
    final aParts = a.split('.').map((s) => int.tryParse(s) ?? 0).toList();
    final bParts = b.split('.').map((s) => int.tryParse(s) ?? 0).toList();

    for (var i = 0; i < 3; i++) {
      final av = i < aParts.length ? aParts[i] : 0;
      final bv = i < bParts.length ? bParts[i] : 0;
      if (av > bv) return true;
      if (av < bv) return false;
    }
    return false;
  }
}

class UpdateInfo {
  final String version;
  final String downloadUrl;
  final String releaseNotes;

  const UpdateInfo({
    required this.version,
    required this.downloadUrl,
    required this.releaseNotes,
  });
}
