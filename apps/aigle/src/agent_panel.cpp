#include "agent_json.hpp"
#include "commands.hpp"
#include "agent_panel.hpp"
#include "localization.hpp"
#include <memory>
#include "adjustments.hpp"
#include <QDialog>
#include <QTextDocument>
#include <QCheckBox>
#include <QUuid>
#include <QCryptographicHash>
#include <QJsonDocument>
#include <QDir>
#include <QDialogButtonBox>
#include <QFileInfo>
#include <QFormLayout>
#include <QKeyEvent>
#include <QScrollArea>
#include <QScrollBar>
#include <QSettings>
#include <QSet>
#include <QSignalBlocker>
#include <QVBoxLayout>
namespace aigle {
namespace {
class TranscriptBrowser final : public QTextBrowser {
  public:
    using QTextBrowser::QTextBrowser;
  protected:
    QVariant loadResource(int, const QUrl&) override { return {}; }
};
class ProviderDialog final : public QDialog {
  public:
    using QDialog::QDialog;
    bool pending = false;
    void reject() override {
        if (!pending) QDialog::reject();
    }
};
}
AgentPanel::AgentPanel(QWidget* parent)
    : QWidget(parent), endpoint_(new QLineEdit(this)), model_(new QLineEdit(this)),
      key_(new QLineEdit(this)), prompt_(new QPlainTextEdit), transcript_(new TranscriptBrowser),
      send_(new QPushButton(tr("Send"))), accept_(new QPushButton(tr("Apply edits"))),
      reject_(new QPushButton(tr("Discard"))), cancel_(new QPushButton(tr("Cancel run"))),
      usage_(new QLabel) {
    QSettings settings;
    endpoint_->setText(settings.value("agent/endpoint").toString());
    model_->setText(settings.value("agent/model").toString());
    key_->setEchoMode(QLineEdit::Password);
    endpoint_->hide();
    model_->hide();
    key_->hide();
    providers_ = agentProvidersFromJson(QJsonDocument::fromJson(settings.value("agent/providers").toByteArray()).array());
    if (providers_.isEmpty() && !endpoint_->text().isEmpty())
        providers_.append({"default", tr("Provider"), endpoint_->text(), model_->text(), {model_->text()}});
    providerId_ = settings.value("agent/activeProvider").toString();
    bool activeExists = false;
    for (const auto& provider : providers_) if (provider.id == providerId_) activeExists = true;
    if (!activeExists) providerId_ = providers_.isEmpty() ? QString() : providers_.first().id;
    auto* root = new QVBoxLayout(this);
    root->setContentsMargins(0, 0, 0, 0);
    root->setSpacing(0);
    auto* scroll = new QScrollArea;
    scroll->setWidgetResizable(true);
    auto* body = new QWidget;
    auto* content = new QVBoxLayout(body);
    content->setContentsMargins(16, 16, 16, 16);
    content->setSpacing(14);
    context_ = new QLabel(tr("Open a photo to work with the editing assistant."));
    context_->setObjectName("muted");
    context_->setWordWrap(true);
    content->addWidget(context_);
    transcript_->setObjectName("agentTranscript");
    transcript_->setOpenExternalLinks(false);
    transcript_->setOpenLinks(false);
    connect(transcript_, &QTextBrowser::anchorClicked, this, [this](const QUrl& url) {
        if (url.scheme() != "profile") return;
        bool valid = false;
        const int index = url.path().toInt(&valid);
        if (!valid || index < 0 || index >= messages_.size()) return;
        if (expandedProfiles_.contains(index)) expandedProfiles_.remove(index);
        else expandedProfiles_.insert(index);
        renderConversation();
    });
    transcript_->setMinimumHeight(170);
    transcript_->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Preferred);
    transcript_->document()->setDefaultStyleSheet(
        "body{font-size:12px;color:#e6e6e6} p{line-height:145%;margin:0 0 14px} "
        ".author{color:#b2c3d4;font-size:11px} .user{border-left:2px solid "
        "#677b90;padding-left:10px}");
    transcript_->setHtml(tr("<p>Describe the look you want. Aigle will inspect rendered previews "
                            "and propose changes for you to review.</p>"));
    content->addWidget(transcript_, 1);
    proposal_ = new QWidget;
    proposal_->setObjectName("agentProposal");
    auto* pl = new QVBoxLayout(proposal_);
    pl->setContentsMargins(12, 12, 12, 12);
    pl->setSpacing(10);
    auto* heading = new QLabel(tr("Proposed adjustments"));
    bindTranslation(heading, [this, heading] { heading->setText(tr("Proposed adjustments")); });
    heading->setObjectName("parameterLabel");
    pl->addWidget(heading);
    preview_ = new QLabel;
    preview_->setObjectName("photoPreview");
    preview_->setAlignment(Qt::AlignCenter);
    preview_->setFixedHeight(100);
    pl->addWidget(preview_);
    summary_ = new QLabel;
    summary_->setObjectName("parameterLabel");
    summary_->setTextFormat(Qt::RichText);
    pl->addWidget(summary_);
    auto* actions = new QHBoxLayout;
    reject_->setObjectName("secondary");
    accept_->setObjectName("primary");
    actions->addWidget(reject_, 1);
    actions->addWidget(accept_, 1);
    pl->addLayout(actions);
    proposal_->hide();
    content->addWidget(proposal_);
    usage_->setObjectName("statusText");
    usage_->setWordWrap(true);
    content->addWidget(usage_);
    content->addStretch();
    scroll->setWidget(body);
    root->addWidget(scroll, 1);
    auto* compose = new QWidget;
    compose->setObjectName("agentCompose");
    auto* composer = new QVBoxLayout(compose);
    composer->setContentsMargins(14, 12, 14, 12);
    composer->setSpacing(8);
    prompt_->setObjectName("agentPrompt");
    prompt_->setPlaceholderText(tr("Describe or refine this edit…"));
    prompt_->setFixedHeight(72);
    prompt_->installEventFilter(this);
    composer->addWidget(prompt_);
    providerChoice_ = new QComboBox;
    providerChoice_->setObjectName("agentProviderChoice");
    providerChoice_->setAccessibleName(tr("Active agent provider"));
    composer->addWidget(providerChoice_);
    auto* controls = new QHBoxLayout;
    modelButton_ = new QToolButton;
    modelButton_->setText(model_->text().isEmpty() ? tr("Configure provider…") : model_->text());
    modelButton_->setObjectName("secondary");
    modelButton_->setMaximumWidth(190);
    controls->addWidget(modelButton_, 1);
    send_->setObjectName("secondary");
    controls->addWidget(send_);
    composer->addLayout(controls);
    cancel_->hide();
    composer->addWidget(cancel_);
    auto* hint = new QLabel(tr("Enter to send · Shift+Enter for a new line"));
    bindTranslation(hint, [this, hint] { hint->setText(tr("Enter to send · Shift+Enter for a new line")); });
    bindTranslation(this, [this] {
        renderSummary();
        send_->setText(tr("Send"));
        accept_->setText(tr("Apply edits"));
        reject_->setText(tr("Discard"));
        cancel_->setText(tr("Cancel run"));
        prompt_->setPlaceholderText(tr("Describe or refine this edit…"));
        providerChoice_->setAccessibleName(tr("Active agent provider"));
        modelButton_->setText(model_->text().isEmpty() ? tr("Configure provider…") : model_->text());
        context_->setText(photo_.isEmpty() ? tr("Open a photo to work with the editing assistant.") :
            tr("Editing %1\nUses the current image and its adjustments.").arg(QFileInfo(photo_).fileName()));
        if (!messages_.isEmpty()) {
            const auto vertical = transcript_->verticalScrollBar()->value();
            const auto horizontal = transcript_->horizontalScrollBar()->value();
            const QSignalBlocker blocker(transcript_);
            renderConversation();
            transcript_->verticalScrollBar()->setValue(vertical);
            transcript_->horizontalScrollBar()->setValue(horizontal);
        }
    });
    hint->setObjectName("statusText");
    composer->addWidget(hint);
    root->addWidget(compose);
    // Protect every focused control in the composer from application shortcuts.
    compose->installEventFilter(this);
    for (auto* child : compose->findChildren<QWidget*>())
        child->installEventFilter(this);
    refreshProviders();
    activateProvider(providerId_);
    connect(providerChoice_, &QComboBox::currentIndexChanged, this, [this] {
        activateProvider(providerChoice_->currentData().toString());
    });
    connect(modelButton_, &QToolButton::clicked, this, [this] {
        ProviderDialog dialog(this);
        dialog.setObjectName("agentProviderDialog");
        dialog.setWindowTitle(tr("Provider & model"));
        auto* form = new QFormLayout(&dialog);
        QString editId = providerId_;
        QString removedId;
        AgentProvider current;
        for (const auto& entry : providers_) if (entry.id == editId) current = entry;
        bool newProfile = editId.isEmpty();
        if (newProfile) editId = QUuid::createUuid().toString(QUuid::WithoutBraces);
        auto discoveredModels = current.models;
        auto* name = new QLineEdit(current.name);
        name->setObjectName("agentProviderName");
        form->addRow(tr("Provider name"), name);
        auto* profileActions = new QWidget;
        auto* profileLayout = new QHBoxLayout(profileActions);
        profileLayout->setContentsMargins(0, 0, 0, 0);
        auto* addProvider = new QPushButton(tr("New provider"));
        addProvider->setObjectName("agentAddProvider");
        auto* removeProvider = new QPushButton(tr("Remove profile"));
        removeProvider->setObjectName("agentRemoveProvider");
        removeProvider->setToolTip(tr("Remove this profile and its saved API key."));
        removeProvider->setEnabled(!newProfile);
        profileLayout->addWidget(addProvider);
        profileLayout->addWidget(removeProvider);
        form->addRow(profileActions);
        auto* endpoint = new QLineEdit(endpoint_->text());
        endpoint->setObjectName("agentEndpoint");
        endpoint->setPlaceholderText("https://provider.example/v1");
        auto* models = new QComboBox;
        models->setObjectName("agentModels");
        models->setEditable(true);
        for (const auto& entry : discoveredModels) models->addItem(entry);
        models->setEditText(model_->text());
        auto* model = models->lineEdit();
        model->setObjectName("agentModel");
        auto* key = new QLineEdit(key_->text());
        key->setObjectName("agentApiKey");
        key->setEchoMode(QLineEdit::Password);
        key->setPlaceholderText(tr("Leave blank to use saved key or AIGLE_AGENT_API_KEY"));
        connect(endpoint, &QLineEdit::textEdited, &dialog, [key, models, &discoveredModels] {
            key->clear();
            discoveredModels = {};
            models->clear();
        });
        form->addRow(tr("Endpoint"), endpoint);
        form->addRow(tr("Vision model"), models);
        auto* discover = new QPushButton(tr("Discover models"));
        discover->setObjectName("agentDiscoverModels");
        form->addRow(discover);
        form->addRow(tr("API key"), key);
        auto* remember = new QCheckBox(tr("Store API key in desktop keyring"));
        remember->setObjectName("agentRememberKey");
        remember->setChecked(true);
        form->addRow(remember);
        auto* forget = new QCheckBox(tr("Remove saved key for this provider and endpoint"));
        forget->setObjectName("agentForgetKey");
        form->addRow(forget);
        connect(forget, &QCheckBox::toggled, &dialog, [key, remember](bool checked) {
            key->setEnabled(!checked);
            remember->setEnabled(!checked);
        });
        auto* status = new QLabel;
        status->setObjectName("agentCredentialStatus");
        auto statusText = std::make_shared<std::function<QString()>>([] { return QString{}; });
        const auto showStatus = [status, statusText](std::function<QString()> text) {
            *statusText = std::move(text);
            status->setText((*statusText)());
        };
        bindTranslation(status, [status, statusText] { status->setText((*statusText)()); });
        status->setWordWrap(true);
        status->setTextFormat(Qt::PlainText);
        form->addRow(status);
        auto* buttons = new QDialogButtonBox(QDialogButtonBox::Save | QDialogButtonBox::Cancel);
        form->addRow(buttons);
        bindTranslation(&dialog, [this, &dialog, form, name, endpoint, models, key,
                                  addProvider, removeProvider, discover, remember, forget] {
            dialog.setWindowTitle(tr("Provider & model"));
            qobject_cast<QLabel*>(form->labelForField(name))->setText(tr("Provider name"));
            qobject_cast<QLabel*>(form->labelForField(endpoint))->setText(tr("Endpoint"));
            qobject_cast<QLabel*>(form->labelForField(models))->setText(tr("Vision model"));
            qobject_cast<QLabel*>(form->labelForField(key))->setText(tr("API key"));
            addProvider->setText(tr("New provider"));
            removeProvider->setText(tr("Remove profile"));
            removeProvider->setToolTip(tr("Remove this profile and its saved API key."));
            discover->setText(tr("Discover models"));
            remember->setText(tr("Store API key in desktop keyring"));
            forget->setText(tr("Remove saved key for this provider and endpoint"));
            key->setPlaceholderText(tr("Leave blank to use saved key or AIGLE_AGENT_API_KEY"));
        });
        const auto requestId = QUuid::createUuid().toString(QUuid::WithoutBraces);
        bool savedSecurely = false;
        bool discovering = false;
        QString discoveryId;
        const auto setFieldsEnabled = [&](bool enabled) {
            buttons->setEnabled(enabled);
            buttons->button(QDialogButtonBox::Save)->setEnabled(enabled);
            endpoint->setEnabled(enabled);
            models->setEnabled(enabled);
            name->setEnabled(enabled);
            addProvider->setEnabled(enabled);
            removeProvider->setEnabled(enabled && !newProfile);
            discover->setEnabled(enabled);
            forget->setEnabled(enabled);
            key->setEnabled(enabled && !forget->isChecked());
            remember->setEnabled(enabled && !forget->isChecked());
        };
        connect(addProvider, &QPushButton::clicked, &dialog, [&] {
            editId = QUuid::createUuid().toString(QUuid::WithoutBraces);
            newProfile = true;
            name->clear();
            endpoint->clear();
            models->clear();
            key->clear();
            discoveredModels = {};
            forget->setChecked(false);
            remember->setChecked(true);
            removeProvider->setEnabled(false);
            showStatus([] { return QString{}; });
            name->setFocus();
        });
        connect(removeProvider, &QPushButton::clicked, &dialog, [&] {
            removedId = editId;
            dialog.pending = true;
            setFieldsEnabled(false);
            showStatus([this] { return tr("Removing the provider's saved API key…"); });
            emit command(commands::AgentCredential{.request_id = requestId, .provider_id = editId, .endpoint = current.endpoint, .action = "delete_provider"});
        });
        connect(discover, &QPushButton::clicked, &dialog, [&] {
            if (endpoint->text().trimmed().isEmpty()) {
                showStatus([this] { return tr("Enter a provider endpoint first."); });
                return;
            }
            discovering = true;
            discoveryId = QUuid::createUuid().toString(QUuid::WithoutBraces);
            setFieldsEnabled(false);
            buttons->setEnabled(true);
            buttons->button(QDialogButtonBox::Save)->setEnabled(false);
            showStatus([this] { return tr("Discovering models…"); });
            emit command(commands::AgentModels{.request_id = discoveryId, .provider_id = editId, .endpoint = endpoint->text().trimmed(), .api_key = key->text()});
        });
        connect(this, &AgentPanel::modelsFinished, &dialog, [&](const ModelDiscovery& result) {
            if (!discovering || result.running || result.requestId != discoveryId) return;
            discovering = false;
            setFieldsEnabled(true);
            if (result.error.has_value()) {
                showStatus([this, error = *result.error] { return tr("Model discovery failed: %1. You can enter a model manually.").arg(error); });
                return;
            }
            const auto selected = model->text();
            discoveredModels = {};
            models->clear();
            for (const auto& entry : result.models) {
                const auto id = entry.trimmed();
                if (id.isEmpty() || discoveredModels.contains(id)) continue;
                discoveredModels.append(id);
                models->addItem(id);
            }
            if (!selected.isEmpty()) models->setEditText(selected);
            showStatus([this, count = discoveredModels.size()] { return count == 0 ? tr("No models returned. Enter a model manually.") :
                tr("%1 models found. Choose a model with image support.").arg(count); });
        });
        connect(this, &AgentPanel::credentialFinished, &dialog, [&](const CredentialResult& result) {
            if (!dialog.pending || result.requestId != requestId) return;
            dialog.pending = false;
            setFieldsEnabled(true);
            if (result.error.has_value()) {
                removedId.clear();
                showStatus([this, error = *result.error] { return tr("Credential update failed: %1").arg(error); });
                return;
            }
            savedSecurely = true;
            dialog.accept();
        });
        connect(buttons, &QDialogButtonBox::accepted, &dialog, [&] {
            if (endpoint->text().trimmed().isEmpty() || model->text().trimmed().isEmpty()) {
                showStatus([this] { return tr("Enter a provider endpoint and model."); });
                return;
            }
            if (!forget->isChecked() && (key->text().isEmpty() || !remember->isChecked())) {
                dialog.accept();
                return;
            }
            dialog.pending = true;
            setFieldsEnabled(false);
            showStatus([this] { return tr("Waiting for the desktop keyring…"); });
            auto request = commands::AgentCredential{.request_id = requestId, .provider_id = editId, .endpoint = endpoint->text().trimmed(), .action = forget->isChecked() ? "delete" : "save"};
            if (!forget->isChecked()) request.api_key = key->text();
            emit command(request);
        });
        connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
        const auto accepted = dialog.exec() == QDialog::Accepted;
        if (discovering) emit command(commands::AgentModelsCancel{.request_id = discoveryId});
        if (accepted && !removedId.isEmpty()) {
            for (qsizetype i = providers_.size(); i-- > 0;)
                if (providers_.at(i).id == removedId) providers_.removeAt(i);
            sessionKeys_.remove(removedId);
            providerId_.clear();
            key_->clear();
            const auto next = providers_.isEmpty() ? QString() : providers_.first().id;
            activateProvider(next);
            refreshProviders();
            persistProviders();
        } else if (accepted) {
            if (editId.isEmpty()) editId = QUuid::createUuid().toString(QUuid::WithoutBraces);
            if (providerId_ != editId && !providerId_.isEmpty()) sessionKeys_.insert(providerId_, key_->text());
            providerId_ = editId;
            endpoint_->setText(endpoint->text().trimmed());
            model_->setText(model->text().trimmed());
            key_->setText(savedSecurely ? QString() : key->text());
            if (!discoveredModels.contains(model_->text())) discoveredModels.append(model_->text());
            const AgentProvider provider{editId, name->text().trimmed().isEmpty() ? endpoint_->text() : name->text().trimmed(),
                endpoint_->text(), model_->text(), discoveredModels};
            bool replaced = false;
            for (qsizetype i = 0; i < providers_.size(); ++i)
                if (providers_.at(i).id == editId) { providers_[i] = provider; replaced = true; break; }
            if (!replaced) providers_.append(provider);
            sessionKeys_.insert(editId, key_->text());
            refreshProviders();
            persistProviders();
            modelButton_->setText(model_->text());
        }
    });
    send_->setEnabled(false);
    accept_->setEnabled(false);
    reject_->setEnabled(false);
    connect(send_, &QPushButton::clicked, this, [this] {
        const auto prompt = prompt_->toPlainText().trimmed();
        if (prompt.isEmpty())
            return;
        if (endpoint_->text().isEmpty() || model_->text().isEmpty()) {
            modelButton_->click();
            return;
        }
        appendMessage("user", prompt);
        prompt_->clear();
        lastMessage_.clear();
        emit command(commands::AgentStart{.provider_id = providerId_, .endpoint = endpoint_->text(), .model = model_->text(), .api_key = key_->text(), .prompt = conversationPrompt()});
    });
    connect(accept_, &QPushButton::clicked, this,
            [this] { emit command(commands::AgentAccept{}); });
    connect(reject_, &QPushButton::clicked, this,
            [this] { emit command(commands::AgentReject{}); });
    connect(cancel_, &QPushButton::clicked, this,
            [this] { emit command(commands::AgentCancel{}); });
}
void AgentPanel::refreshProviders() {
    const QSignalBlocker block(providerChoice_);
    providerChoice_->clear();
    for (const auto& entry : providers_) {
        const auto& provider = entry;
        providerChoice_->addItem(provider.name, provider.id);
    }
    if (providers_.isEmpty()) providerChoice_->addItem(tr("No provider configured"), QString());
    providerChoice_->setCurrentIndex(qMax(0, providerChoice_->findData(providerId_)));
}
void AgentPanel::activateProvider(const QString& id) {
    if (!providerId_.isEmpty()) sessionKeys_.insert(providerId_, key_->text());
    providerId_ = id;
    endpoint_->clear();
    model_->clear();
    key_->setText(sessionKeys_.value(id));
    for (const auto& entry : providers_) {
        const auto& provider = entry;
        if (provider.id != id) continue;
        endpoint_->setText(provider.endpoint);
        model_->setText(provider.model);
        break;
    }
    modelButton_->setText(model_->text().isEmpty() ? tr("Configure provider…") : model_->text());
    persistProviders();
}
void AgentPanel::persistProviders() {
    QSettings settings;
    settings.setValue("agent/providers", QJsonDocument(toJson(providers_)).toJson(QJsonDocument::Compact));
    settings.setValue("agent/activeProvider", providerId_);
    settings.setValue("agent/endpoint", endpoint_->text());
    settings.setValue("agent/model", model_->text());
}
void AgentPanel::setPhoto(const QString& path, const QString& copy, const QString& stableIdentity) {
    const auto identity = path.isEmpty() ? QString() :
        (stableIdentity.isEmpty() ? QDir::cleanPath(QFileInfo(path).absoluteFilePath()) : "catalog:" + stableIdentity) + "\n" + copy;
    context_->setText(path.isEmpty() ? tr("Open a photo to work with the editing assistant.") :
        tr("Editing %1\nUses the current image and its adjustments.").arg(QFileInfo(path).fileName()));
    const auto key = identity.isEmpty() ? QString() : "agent/conversations/" +
        QString::fromLatin1(QCryptographicHash::hash(identity.toUtf8(), QCryptographicHash::Sha256).toHex());
    photo_ = path;
    if (conversationKey_ == key)
        return;
    conversationKey_ = key;
    messages_ = key.isEmpty() ? QList<AgentMessage>{} :
        agentMessagesFromJson(QJsonDocument::fromJson(QSettings().value(key).toByteArray()).array());
    lastMessage_.clear();
    runId_ = -1;
    receivedEvents_ = 0;
    expandedProfiles_.clear();
    renderConversation();
    prompt_->clear();
    usageStats_ = {};
    summaryParams_ = {};
    conversationSaveFailed_ = false;
    usage_->clear();
    context_->setText(path.isEmpty() ? tr("Open a photo to work with the editing assistant.") :
        tr("Editing %1\nUses the current image and its adjustments.").arg(QFileInfo(path).fileName()));
    proposal_->hide();
}
void AgentPanel::renderConversation() {
    transcript_->clear();
    for (int index = 0; index < messages_.size(); ++index) {
        const auto message = messages_[index];
        const auto author = message.role == "user" ? tr("You") : tr("Aigle assistant");
        QTextDocument content;
        content.setMarkdown(message.text, QTextDocument::MarkdownNoHTML);
        transcript_->append(QString("<p><b>%1</b></p>").arg(author.toHtmlEscaped()));
        transcript_->append(content.toHtml());
        const auto imageBytes = message.image;
        if (!imageBytes.isEmpty()) {
            const auto image = QImage::fromData(imageBytes);
            if (!image.isNull()) {
                const QUrl url(QString("agent-preview:%1").arg(index));
                const auto preview = image.scaled(240, 180, Qt::KeepAspectRatio, Qt::SmoothTransformation);
                transcript_->document()->addResource(QTextDocument::ImageResource, url, preview);
                transcript_->append(QString("<p><img src='%1' width='%2' height='%3'></p>")
                    .arg(url.toString()).arg(preview.width()).arg(preview.height()));
            }
        }
        const auto profile = message.profile;
        if (!profile.isEmpty()) {
            transcript_->append(QString("<p><a href='profile:%1'>%2</a></p>").arg(index)
                .arg(expandedProfiles_.contains(index) ? tr("Hide adjustments") : tr("Show adjustments")));
            if (expandedProfiles_.contains(index)) {
                QString table = "<table>";
                for (auto it = profile.begin(); it != profile.end(); ++it) {
                    const auto value = QJsonDocument(QJsonArray{it.value()}).toJson(QJsonDocument::Compact);
                    table += QString("<tr><td>%1</td><td>%2</td></tr>").arg(it.key().toHtmlEscaped(),
                        QString::fromUtf8(value.mid(1, value.size() - 2)).toHtmlEscaped());
                }
                transcript_->append(table + "</table>");
            }
        }
    }
}
void AgentPanel::persistConversation() {
    if (conversationKey_.isEmpty()) return;
    QSettings settings;
    settings.setValue(conversationKey_, QJsonDocument(toJson(messages_)).toJson(QJsonDocument::Compact));
    settings.sync();
    renderConversation();
    conversationSaveFailed_ = settings.status() != QSettings::NoError;
    renderSummary();
}
void AgentPanel::appendMessage(const QString& role, const QString& text) {
    if (conversationKey_.isEmpty()) return;
    messages_.append({role, text, {}, {}});
    persistConversation();
}
QString AgentPanel::conversationPrompt() const {
    QStringList turns;
    for (const auto& value : messages_) {
        const auto& message = value;
        const auto role = message.role;
        if (role == "user" || role == "assistant")
            turns.append((role == "user" ? "User: " : "Assistant: ") + message.text);
    }
    return turns.join("\n\n");
}
void AgentPanel::setPreview(const QImage& image) {
    preview_->setPixmap(
        QPixmap::fromImage(image).scaled(250, 100, Qt::KeepAspectRatio, Qt::SmoothTransformation));
}
bool AgentPanel::eventFilter(QObject* watched, QEvent* event) {
    if (event->type() == QEvent::ShortcutOverride) {
        event->accept();
        return true;
    }
    if (watched == prompt_ && event->type() == QEvent::KeyPress) {
        auto* key = static_cast<QKeyEvent*>(event);
        if ((key->key() == Qt::Key_Return || key->key() == Qt::Key_Enter) &&
            !(key->modifiers() & Qt::ShiftModifier)) {
            if (send_->isEnabled())
                send_->click();
            return true;
        }
    }
    return QWidget::eventFilter(watched, event);
}
void AgentPanel::setState(const AgentState& state, bool photoOpen) {
    if (state.runId.has_value()) {
        const auto run = *state.runId;
        if (run != runId_) { runId_ = run; receivedEvents_ = 0; lastMessage_.clear(); }
        const auto events = state.events;
        bool changed = false;
        while (receivedEvents_ < events.size()) {
            const auto event = events[receivedEvents_++];
            messages_.append(event);
            if (!event.text.isEmpty()) lastMessage_ = event.text;
            changed = true;
        }
        if (changed) persistConversation();
    }
    const bool running = state.running,
               proposal = state.proposal;
    send_->setEnabled(photoOpen && !running);
    cancel_->setVisible(running);
    accept_->setEnabled(proposal);
    reject_->setEnabled(proposal);
    proposal_->setVisible(proposal);
    const auto message = state.error.has_value() ? *state.error
                                                 : state.message;
    if (!running && !message.isEmpty() && message != lastMessage_) {
        appendMessage(state.error.has_value() ? "error" : "assistant", message);
        lastMessage_ = message;
    }
    if (proposal) summaryParams_ = state.params;
    const auto stats = state.stats;
    if (stats.has_value()) usageStats_ = stats;
    renderSummary();
}
void AgentPanel::renderSummary() {
    if (!summaryParams_.isEmpty()) {
        const auto& params = summaryParams_;
        QString html = "<table width='100%' cellspacing='5'>";
        for (const auto& entry :
             std::array<std::pair<QString, QString>, 4>{{{uiText("Exposure"), "light.exposure"},
                                                         {uiText("Highlights"), "light.highlights"},
                                                         {uiText("Shadows"), "light.shadows"},
                                                         {uiText("Temperature"), "wb.temperature"}}})
            html += QString("<tr><td>%1</td><td align='right'>%2%3</td></tr>")
                        .arg(entry.first,
                             QString::number(valueAt(params, entry.second).toDouble(), 'f',
                                             entry.second == "light.exposure" ? 2 : 0),
                             entry.second == "light.exposure"   ? " EV"
                             : entry.second == "wb.temperature" ? " K"
                                                                : "");
        summary_->setText(html + "</table>");
    }
    if (conversationSaveFailed_) {
        usage_->setText(tr("Conversation could not be saved. Check available storage and permissions."));
        return;
    }
    const auto& stats = usageStats_;
    if (stats.has_value())
        usage_->setText(tr("%1 requests · %2 s\n%3 input / %4 output tokens")
                            .arg(stats->requests)
                            .arg(stats->seconds, 0, 'f', 1)
                            .arg(stats->inputTokens)
                            .arg(stats->outputTokens));
}
} // namespace aigle
