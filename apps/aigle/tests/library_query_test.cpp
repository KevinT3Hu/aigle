#include "library_json.hpp"
#include "library_query.hpp"
#include <QJsonDocument>
#include <QSettings>
#include <QTemporaryDir>
#include <QTest>
using namespace aigle;
static bool matchesJson(const QString& path, const QJsonObject& photo, const QJsonObject& filter) {
    return libraryMatches(path, libraryPhotoFromJson(photo), libraryFilterFromJson(filter));
}
static bool lessJson(const QString& a, const QJsonObject& am, const QString& b, const QJsonObject& bm, const QString& key, bool descending) {
    return libraryLess(a, libraryPhotoFromJson(am), b, libraryPhotoFromJson(bm), key, descending);
}
static QJsonObject newFolderJson(const QJsonObject& filter) {
    return toJson(libraryFilterForNewFolder(libraryFilterFromJson(filter)));
}
class LibraryQueryTest final : public QObject {
    Q_OBJECT
    QJsonObject photo() const {
        return {{"meta_loaded", true},
                {"facts_loaded", true},
                {"rating", 4},
                {"flag", "picked"},
                {"label", "Red"},
                {"tags", QJsonArray{"Places > France > Sea"}},
                {"keywords", QJsonArray{"Places", "France", "Sea"}},
                {"iptc", QJsonObject{{"caption", "Summer holiday"}}},
                {"camera", "Canon EOS 6D"},
                {"lens", "24-70mm"},
                {"captured", "2024-05-06 10:00:00"}};
    }
  private slots:
    void collapsedStacksFollowFilterAndOrder() {
        QSet<QString> seen;
        QVERIFY(!libraryStackVisible(false, "burst", true, seen));
        QVERIFY(libraryStackVisible(true, "burst", true, seen));
        QVERIFY(!libraryStackVisible(true, "burst", true, seen));
        QVERIFY(libraryStackVisible(true, "", true, seen));
        QVERIFY(libraryStackVisible(true, "other", true, seen));
        QVERIFY(libraryStackVisible(true, "burst", false, seen));
        seen.clear(); // A new sort order chooses a new first matching member.
        QVERIFY(libraryStackVisible(true, "burst", true, seen));
    }
    void metadataAndExcludedTerms() {
        for (const auto* query :
             {"BEACH sea canon", "summer -portrait", "2024 24-70", "france RED"})
            QVERIFY(matchesJson("/photos/beach.nef", photo(), {{"query", query}}));
        for (const auto* query : {"sea -canon", "beach mountain", "2023", "-holiday"})
            QVERIFY(!matchesJson("/photos/beach.nef", photo(), {{"query", query}}));
        QVERIFY(matchesJson("/photos/beach.nef", {}, {{"query", "beach"}}));
    }
    void metadataFiltersAndMissingValues() {
        QJsonObject filter{{"rating", QJsonObject{{"AtLeast", 4}}},
                           {"label", QJsonObject{{"Is", "red"}}},
                           {"camera", "Canon EOS 6D"},
                           {"lens", "24-70mm"},
                           {"date", "2024-05"},
                           {"keyword", "SEA"}};
        QVERIFY(matchesJson("beach.nef", photo(), filter));
        QVERIFY(!matchesJson("beach.nef", {}, filter));
        filter["keyword"] = "France > Sea";
        QVERIFY(!matchesJson("beach.nef", photo(), filter));
        filter["keyword"] = "Sea";
        filter["date"] = "2023";
        QVERIFY(!matchesJson("beach.nef", photo(), filter));
        QVERIFY(matchesJson("beach.nef", photo(), {{"rating", "Picked"}}));
        auto rejected = photo();
        rejected["flag"] = "rejected";
        QVERIFY(!matchesJson("beach.nef", rejected, {{"rating", QJsonObject{{"AtLeast", 4}}}}));
        QVERIFY(matchesJson("beach.nef", rejected, {{"rating", "Rejected"}}));
        rejected["rating"] = -1;
        rejected["flag"] = "none";
        QVERIFY(matchesJson("beach.nef", rejected, {{"rating", "Rejected"}}));
        QVERIFY(!matchesJson("beach.nef", rejected, {{"rating", "Unrated"}}));
        QVERIFY(!matchesJson("beach.nef", photo(), {{"label", "None"}}));
    }
    void changingFoldersDropsOnlyFolderSpecificCriteria() {
        const QJsonObject filter{{"query", "winter -portrait"},
                                 {"rating", "Picked"},
                                 {"label", QJsonObject{{"Is", "custom"}}},
                                 {"camera", "Canon"},
                                 {"lens", "24-70mm"},
                                 {"date", "2024-05"},
                                 {"keyword", "Sea"}};
        const auto next = newFolderJson(filter);
        QCOMPARE(next["query"], filter["query"]);
        QCOMPARE(next["rating"], filter["rating"]);
        QCOMPARE(next["label"], filter["label"]);
        for (const auto* key : {"camera", "lens", "date", "keyword"})
            QVERIFY(!next.contains(key));
        QCOMPARE(newFolderJson(next), next);
    }
    void sortingUnknownsAndTies() {
        const auto a = photo();
        auto b = a;
        b["rating"] = 5;
        b["captured"] = "2025-01-01 00:00:00";
        for (const auto* key : {"Rating", "Captured"}) {
            QVERIFY(lessJson("a", a, "b", b, key, false));
            QVERIFY(lessJson("b", b, "a", a, key, true));
            QVERIFY(lessJson("a", a, "unknown", {}, key, true));
            QVERIFY(lessJson("a", a, "unknown", {}, key, false));
            QVERIFY(lessJson("a", a, "b", a, key, true));
        }
        QVERIFY(lessJson("a", {{"modified", 1}}, "b", {{"modified", 2}}, "Modified", false));
        QVERIFY(lessJson("b", {}, "A", {}, "Name", true));
        QVERIFY(!lessJson("a", a, "a", a, "Rating", true));
    }
    void savedSearchValidationAndPersistence() {
        const auto search =
            QJsonDocument::fromJson(
                R"({"name":"Beaches","filter":{"query":"sea -portrait","rating":{"AtLeast":4},"label":{"Is":"red"},"camera":"Canon EOS 6D","lens":null,"date":"2024","keyword":"Sea"},"sort":{"key":"Captured","descending":true}})")
                .object();
        QVERIFY(validLibrarySearch(search));
        QVERIFY(matchesJson("beach.nef", photo(), search["filter"].toObject()));
        QTemporaryDir dir;
        const auto path = dir.filePath("searches.ini");
        {
            QSettings settings(path, QSettings::IniFormat);
            settings.setValue("library/savedSearches", QVariantList{search.toVariantMap()});
            settings.sync();
            QCOMPARE(settings.status(), QSettings::NoError);
        }
        QSettings settings(path, QSettings::IniFormat);
        QCOMPARE(QJsonObject::fromVariantMap(
                     settings.value("library/savedSearches").toList().first().toMap()),
                 search);
        auto invalid = search;
        invalid["sort"] = QJsonObject{{"key", "Unsupported"}, {"descending", true}};
        QVERIFY(!validLibrarySearch(invalid));
        invalid = search;
        auto filter = invalid["filter"].toObject();
        filter["rating"] = QJsonObject{{"AtLeast", 7}};
        invalid["filter"] = filter;
        QVERIFY(!validLibrarySearch(invalid));
        filter["rating"] = "Any";
        filter["unknownCriterion"] = true;
        invalid["filter"] = filter;
        QVERIFY(!validLibrarySearch(invalid));
    }
};
QTEST_GUILESS_MAIN(LibraryQueryTest)
#include "library_query_test.moc"
