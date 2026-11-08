#pragma once
#include "editor_state.hpp"
#include "export_input.hpp"
#include "library_types.hpp"
#include <variant>
namespace aigle {
struct ProfileResult { QString name, cameraModel; };
struct ToolResult { std::optional<qint64> createdMask; };
struct CatalogRestoreResult {
    QString recovery, error;
    bool restored = false;
    std::optional<QString> rollbackError;
};
struct OpenResult {
    QString path;
    qint64 copy = 0, id = 0;
    bool develop = true;
    QStringList warnings;
};
struct FileActionResult {
    PhotoIdentity photo;
    QString action;
    std::optional<QString> error, reopenError;
    bool ok = false, closed = false;
    std::optional<OpenResult> opened;
};
struct PhotoMetadata {
    PhotoIdentity identity;
    LibraryPhoto facts;
    QString stack;
    std::optional<QString> error;
};
struct BatchResult {
    QList<PhotoMetadata> updated;
    QList<QPair<QString, QString>> failures;
};
struct CopyRecord { qint64 id = 0; QString path, name; };
struct CreateCopiesResult { QList<CopyRecord> copies; QList<QPair<QString, QString>> errors; };
struct AlbumRecord { qint64 id = 0; QString name; QStringList paths; };
struct AlbumChoices { QList<AlbumRecord> albums; QString action = "open"; QStringList paths; };
struct ExportInputsResult { QList<ExportInput> inputs; std::optional<QString> error; };
struct SchemaDocument { QJsonObject schema; };
using ReplyData = std::variant<std::monostate, ProfileResult, ToolResult, CatalogRestoreResult,
    OpenResult, FileActionResult, PhotoMetadata, BatchResult, QList<CopyRecord>, CopyRecord,
    CreateCopiesResult, AlbumRecord, AlbumChoices, ExportInputsResult, SchemaDocument,
    QStringList, qint64, RetouchResult, ScopeResult, CredentialResult, ModelDiscovery,
    ExportProgress, DuplicateResult>;
struct EngineReply {
    QString command;
    ReplyData data;
    template<class T> const T& get() const { return std::get<T>(data); }
};
}
Q_DECLARE_METATYPE(aigle::EngineReply)
