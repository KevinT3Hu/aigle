#include "command_json.hpp"
#include "commands.hpp"
#include "export_dialog.hpp"
#include "theme.hpp"
#include "localization.hpp"
#include <QSignalBlocker>
#include <QDir>
#include <QColorDialog>
#include <QInputDialog>
#include <QJsonDocument>
#include <QMessageBox>
#include <QFileDialog>
#include <QFileInfo>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QPushButton>
#include <QScrollArea>
#include <QSettings>
#include <QSet>
#include <QVBoxLayout>

namespace aigle {
ExportDialog::ExportDialog(const QString& source, const QImage& preview, QSize sourceSize,
                           const QList<ExportFormat>& formats, QWidget* parent, QList<ExportInput> inputs)
    : QDialog(parent), inputs_(std::move(inputs)), source_(source), sourceSize_(sourceSize), formats_(formats) {
    const auto alreadyDng = [](const QString& path) {
        return QFileInfo(path).suffix().compare("dng", Qt::CaseInsensitive) == 0;
    };
    const bool canConvertDng = inputs_.isEmpty() ? !alreadyDng(source_) :
        std::all_of(inputs_.begin(), inputs_.end(), [&](const ExportInput& input) {
            const auto path = input.source.toObject()["File"].toString(
                input.path.isNull() ? input.name : input.path);
            return input.raw && !alreadyDng(path);
        });
    if (!canConvertDng)
        for (int i = int(formats_.size()) - 1; i >= 0; --i)
            if (formats_[i].format == "Dng") formats_.removeAt(i);
    setObjectName("exportDialog");
    setWindowTitle(inputs_.size() > 1 ? tr("Export %1 photos").arg(inputs_.size()) : tr("Export photo"));
    setWindowFlag(Qt::FramelessWindowHint);
    setModal(true);
    setFixedWidth(680);
    auto* root = new QVBoxLayout(this);
    root->setContentsMargins(22, 18, 22, 14);
    root->setSpacing(16);
    auto* heading = new QHBoxLayout;
    auto* title = new QLabel(windowTitle());
    title->setStyleSheet("font-size:17px;font-weight:500");
    heading->addWidget(title, 1);
    auto* close = new QPushButton(QString::fromUtf8("×"));
    close->setFixedSize(24, 24);
    close->setAccessibleName(tr("Close export"));
    heading->addWidget(close);
    connect(close, &QPushButton::clicked, this, &QDialog::reject);
    root->addLayout(heading);
    auto* subtitle = new QLabel(tr("Create finished copies. Your original stays untouched."));
    bindTranslation(subtitle, [subtitle] { subtitle->setText(tr("Create finished copies. Your original stays untouched.")); });
    subtitle->setObjectName("muted");
    root->addWidget(subtitle);
    auto* body = new QHBoxLayout;
    body->setSpacing(24);
    auto* formBody = new QWidget;
    auto* form = new QVBoxLayout(formBody);
    form->setSpacing(12);
    auto* formScroll = new QScrollArea;
    formScroll->setWidgetResizable(true);
    formScroll->setWidget(formBody);
    formScroll->setMinimumHeight(480);
    formScroll->setMaximumHeight(650);
    body->addWidget(formScroll, 1);
    const auto field = [](QVBoxLayout* layout, const char* label, QWidget* control) {
        auto* l = new QLabel;
        bindTranslation(l, [l, label] { l->setText(tr(label)); });
        l->setObjectName("parameterLabel");
        l->setBuddy(control);
        layout->addWidget(l);
        control->setMinimumHeight(30);
        layout->addWidget(control);
    };
    auto* preset = new QComboBox;
    preset->addItems(
        {tr("Web · sRGB JPEG"), tr("Full resolution · TIFF"), tr("HDR + SDR delivery")});
    field(form, QT_TR_NOOP("Preset"), preset);
    auto savedPresets = QJsonDocument::fromJson(QSettings().value("export/presets").toByteArray()).object();
    for (auto it = savedPresets.begin(); it != savedPresets.end(); ++it)
        preset->addItem(it.key(), it.value().toObject());
    auto* presetButtons = new QHBoxLayout;
    auto* savePreset = new QPushButton(tr("Save preset…"));
    bindTranslation(savePreset, [savePreset] { savePreset->setText(tr("Save preset…")); });
    auto* deletePreset = new QPushButton(tr("Delete preset"));
    bindTranslation(deletePreset, [deletePreset] { deletePreset->setText(tr("Delete preset")); });
    presetButtons->addWidget(savePreset);
    presetButtons->addWidget(deletePreset);
    form->addLayout(presetButtons);
    connect(savePreset, &QPushButton::clicked, this, [this, preset] {
        bool ok = false;
        const auto name = QInputDialog::getText(this, tr("Save export preset"), tr("Name"), QLineEdit::Normal, {}, &ok).trimmed();
        if (!ok || name.isEmpty()) return;
        auto values = QJsonDocument::fromJson(QSettings().value("export/presets").toByteArray()).object();
        if (values.contains(name) && QMessageBox::question(this, tr("Replace preset"), tr("Replace %1?").arg(name)) != QMessageBox::Yes) return;
        values[name] = settings();
        QSettings store;
        store.setValue("export/presets", QJsonDocument(values).toJson(QJsonDocument::Compact));
        store.sync();
        if (store.status() != QSettings::NoError) { QMessageBox::warning(this, tr("Export presets"), tr("Could not save presets.")); return; }
        int index = -1;
        for (int i = 3; i < preset->count(); ++i) if (preset->itemText(i) == name) index = i;
        if (index < 0) { preset->addItem(name, values[name].toObject()); index = preset->count() - 1; }
        else preset->setItemData(index, values[name].toObject());
        preset->setCurrentIndex(index);
    });
    connect(deletePreset, &QPushButton::clicked, this, [this, preset] {
        if (preset->currentIndex() < 3) return;
        if (QMessageBox::question(this, tr("Delete preset"), tr("Delete %1?").arg(preset->currentText())) != QMessageBox::Yes) return;
        auto values = QJsonDocument::fromJson(QSettings().value("export/presets").toByteArray()).object();
        values.remove(preset->currentText());
        QSettings store;
        store.setValue("export/presets", QJsonDocument(values).toJson(QJsonDocument::Compact));
        store.sync();
        if (store.status() == QSettings::NoError) preset->removeItem(preset->currentIndex());
        else QMessageBox::warning(this, tr("Export presets"), tr("Could not save presets."));
    });
    destination_ = new QLineEdit(
        QSettings().value("export/destination", QFileInfo(source).absolutePath()).toString());
    destination_->setObjectName("exportDestination");
    auto* destRow = new QWidget;
    auto* dl = new QHBoxLayout(destRow);
    dl->setContentsMargins(0, 0, 0, 0);
    dl->setSpacing(6);
    dl->addWidget(destination_, 1);
    auto* browse = new QPushButton(tr("…"));
    bindTranslation(browse, [browse] { browse->setText(tr("…")); });
    browse->setFixedWidth(28);
    bindTranslation(browse, [browse] { browse->setAccessibleName(tr("Choose export destination")); });
    dl->addWidget(browse);
    field(form, QT_TR_NOOP("Destination"), destRow);
    connect(browse, &QPushButton::clicked, this, [this] {
        const auto p =
            QFileDialog::getExistingDirectory(this, tr("Export destination"), destination_->text());
        if (!p.isEmpty())
            destination_->setText(p);
    });
    name_ = new QLineEdit("{name}-edit");
    name_->setObjectName("exportNaming");
    name_->setToolTip(tr("{name}: photo or copy name; {seq}: sequence number. Duplicate names receive a numeric suffix."));
    bindTranslation(name_, [this] { name_->setToolTip(tr("{name}: photo or copy name; {seq}: sequence number. Duplicate names receive a numeric suffix.")); });
    field(form, QT_TR_NOOP("File naming"), name_);
    auto* outputRow = new QHBoxLayout;
    outputRow->setSpacing(12);
    auto* f1 = new QVBoxLayout;
    auto* f2 = new QVBoxLayout;
    outputRow->addLayout(f1, 1);
    outputRow->addLayout(f2, 1);
    format_ = new QComboBox;
    format_->setObjectName("exportFormat");
    for (const auto& f : formats_)
        format_->addItem(f.name);
    quality_ = new QSpinBox;
    quality_->setRange(1, 100);
    quality_->setValue(90);
    field(f1, QT_TR_NOOP("Format"), format_);
    field(f2, QT_TR_NOOP("Quality"), quality_);
    form->addLayout(outputRow);
    auto* sizeRow = new QHBoxLayout;
    sizeRow->setSpacing(12);
    auto* s1 = new QVBoxLayout;
    auto* s2 = new QVBoxLayout;
    sizeRow->addLayout(s1, 1);
    sizeRow->addLayout(s2, 1);
    size_ = new QComboBox;
    size_->addItems({tr("Resize long edge"), tr("Full resolution")});
    bindTranslation(size_, [this] {
        const QSignalBlocker blocker(size_);
        const QStringList labels{tr("Resize long edge"), tr("Full resolution")};
        for (int i = 0; i < labels.size(); ++i) size_->setItemText(i, labels[i]);
    });
    longEdge_ = new QSpinBox;
    longEdge_->setObjectName("exportLongEdge");
    longEdge_->setRange(1, 65535);
    longEdge_->setValue(2400);
    longEdge_->setSuffix(tr(" px"));
    bindTranslation(longEdge_, [this] { longEdge_->setSuffix(tr(" px")); });
    connect(longEdge_, &QSpinBox::valueChanged, this, &ExportDialog::updatePreview);
    profile_ = new QComboBox;
    profile_->addItems({"sRGB", "Display P3", "Adobe RGB", "ProPhoto RGB", "Rec.2020"});
    field(s1, QT_TR_NOOP("Size"), size_);
    field(s2, QT_TR_NOOP("Color space"), profile_);
    form->addLayout(sizeRow);
    field(form, QT_TR_NOOP("Long edge"), longEdge_);
    auto* finishing = new QLabel(tr("Metadata & finishing"));
    bindTranslation(finishing, [finishing] { finishing->setText(tr("Metadata & finishing")); });
    finishing->setContentsMargins(0, 8, 0, 4);
    form->addWidget(finishing);
    auto* advanced = new QFormLayout;
    advanced->setVerticalSpacing(10);
    metadata_ = new QComboBox;
    metadata_->addItems({tr("Copyright only"), tr("All metadata"), tr("None")});
    bindTranslation(metadata_, [this] {
        const QSignalBlocker blocker(metadata_);
        const QStringList labels{tr("Copyright only"), tr("All metadata"), tr("None")};
        for (int i = 0; i < labels.size(); ++i) metadata_->setItemText(i, labels[i]);
    });
    advanced->addRow(tr("Metadata"), metadata_);
    bindTranslation(metadata_, [this, advanced] { qobject_cast<QLabel*>(advanced->labelForField(metadata_))->setText(tr("Metadata")); });
    artist_ = new QLineEdit;
    artist_->setObjectName("exportArtist");
    copyright_ = new QLineEdit;
    copyright_->setObjectName("exportCopyright");
    artist_->setPlaceholderText(tr("Keep source artist"));
    bindTranslation(artist_, [this] { artist_->setPlaceholderText(tr("Keep source artist")); });
    copyright_->setPlaceholderText(tr("Keep source copyright"));
    bindTranslation(copyright_, [this] { copyright_->setPlaceholderText(tr("Keep source copyright")); });
    advanced->addRow(tr("Artist"), artist_);
    bindTranslation(artist_, [this, advanced] { qobject_cast<QLabel*>(advanced->labelForField(artist_))->setText(tr("Artist")); });
    advanced->addRow(tr("Copyright"), copyright_);
    bindTranslation(copyright_, [this, advanced] { qobject_cast<QLabel*>(advanced->labelForField(copyright_))->setText(tr("Copyright")); });
    sharpen_ = new QComboBox;
    sharpen_->addItems({tr("Screen · standard"), tr("Off"), tr("Low"), tr("High")});
    bindTranslation(sharpen_, [this] {
        const QSignalBlocker blocker(sharpen_);
        const QStringList labels{tr("Screen · standard"), tr("Off"), tr("Low"), tr("High")};
        for (int i = 0; i < labels.size(); ++i) sharpen_->setItemText(i, labels[i]);
    });
    advanced->addRow(tr("Output sharpening"), sharpen_);
    bindTranslation(sharpen_, [this, advanced] { qobject_cast<QLabel*>(advanced->labelForField(sharpen_))->setText(tr("Output sharpening")); });
    watermark_ = new QComboBox;
    watermark_->addItems({tr("None"), tr("Text"), tr("Image")});
    bindTranslation(watermark_, [this] {
        const QSignalBlocker blocker(watermark_);
        const QStringList labels{tr("None"), tr("Text"), tr("Image")};
        for (int i = 0; i < labels.size(); ++i) watermark_->setItemText(i, labels[i]);
    });
    watermark_->setObjectName("exportWatermarkKind");
    advanced->addRow(tr("Watermark"), watermark_);
    bindTranslation(watermark_, [this, advanced] { qobject_cast<QLabel*>(advanced->labelForField(watermark_))->setText(tr("Watermark")); });
    form->addLayout(advanced);
    watermarkText_ = new QLineEdit;
    watermarkText_->setObjectName("exportWatermarkText");
    watermarkText_->setPlaceholderText(tr("Watermark text"));
    bindTranslation(watermarkText_, [this] { watermarkText_->setPlaceholderText(tr("Watermark text")); });
    watermarkText_->hide();
    form->addWidget(watermarkText_);
    connect(watermark_, qOverload<int>(&QComboBox::currentIndexChanged), this, [this](int i) {
        watermarkText_->setVisible(i == 1);
        adjustSize();
    });
    watermarkImage_ = new QLineEdit;
    watermarkImage_->setObjectName("exportWatermarkImage");
    auto* imageBrowse = new QPushButton(tr("Choose watermark image…"));
    bindTranslation(imageBrowse, [imageBrowse] { imageBrowse->setText(tr("Choose watermark image…")); });
    form->addWidget(watermarkImage_);
    form->addWidget(imageBrowse);
    connect(imageBrowse, &QPushButton::clicked, this, [this] {
        const auto path = QFileDialog::getOpenFileName(this, tr("Watermark image"), {}, tr("Images (*.png *.svg *.jpg *.webp);;All files (*)"));
        if (!path.isEmpty()) { watermarkImage_->setText(path); watermark_->setCurrentIndex(2); }
    });
    auto* watermarkForm = new QFormLayout;
    watermarkSize_ = new QDoubleSpinBox;
    watermarkSize_->setObjectName("exportWatermarkSize");
    watermarkOpacity_ = new QDoubleSpinBox;
    watermarkOpacity_->setObjectName("exportWatermarkOpacity");
    watermarkMargin_ = new QDoubleSpinBox;
    watermarkMargin_->setObjectName("exportWatermarkMargin");
    for (auto* spin : {watermarkSize_, watermarkOpacity_, watermarkMargin_}) { spin->setRange(0, 100); spin->setSuffix(tr(" %")); spin->setDecimals(1); }
    watermarkSize_->setMinimum(1);
    watermarkSize_->setMaximum(25);
    watermarkSize_->setValue(4);
    watermarkOpacity_->setValue(60);
    watermarkMargin_->setValue(3);
    watermarkAnchor_ = new QComboBox;
    const QStringList anchors{"TopLeft", "Top", "TopRight", "Left", "Center", "Right", "BottomLeft", "Bottom", "BottomRight"};
    const QStringList anchorLabels{tr("Top left"), tr("Top"), tr("Top right"), tr("Left"), tr("Center"), tr("Right"), tr("Bottom left"), tr("Bottom"), tr("Bottom right")};
    for (int i = 0; i < anchors.size(); ++i) watermarkAnchor_->addItem(anchorLabels[i], anchors[i]);
    watermarkAnchor_->setCurrentIndex(8);
    bindTranslation(watermarkAnchor_, [this] {
        const QSignalBlocker blocker(watermarkAnchor_);
        const QStringList labels{tr("Top left"), tr("Top"), tr("Top right"), tr("Left"), tr("Center"), tr("Right"), tr("Bottom left"), tr("Bottom"), tr("Bottom right")};
        for (int i = 0; i < labels.size(); ++i) watermarkAnchor_->setItemText(i, labels[i]);
    });
    watermarkForm->addRow(tr("Watermark size"), watermarkSize_);
    bindTranslation(watermarkSize_, [this, watermarkForm] { qobject_cast<QLabel*>(watermarkForm->labelForField(watermarkSize_))->setText(tr("Watermark size")); });
    watermarkForm->addRow(tr("Opacity"), watermarkOpacity_);
    bindTranslation(watermarkOpacity_, [this, watermarkForm] { qobject_cast<QLabel*>(watermarkForm->labelForField(watermarkOpacity_))->setText(tr("Opacity")); });
    watermarkForm->addRow(tr("Margin"), watermarkMargin_);
    bindTranslation(watermarkMargin_, [this, watermarkForm] { qobject_cast<QLabel*>(watermarkForm->labelForField(watermarkMargin_))->setText(tr("Margin")); });
    watermarkForm->addRow(tr("Position"), watermarkAnchor_);
    bindTranslation(watermarkAnchor_, [this, watermarkForm] { qobject_cast<QLabel*>(watermarkForm->labelForField(watermarkAnchor_))->setText(tr("Position")); });
    auto* color = new QPushButton(tr("Choose text color…"));
    bindTranslation(color, [color] { color->setText(tr("Choose text color…")); });
    watermarkForm->addRow(color);
    connect(color, &QPushButton::clicked, this, [this] {
        const auto selected = QColorDialog::getColor(watermarkColor_, this, tr("Watermark color"));
        if (selected.isValid()) watermarkColor_ = selected;
    });
    form->addLayout(watermarkForm);
    paired_ = new QCheckBox(tr("Also export an SDR JPEG"));
    form->addWidget(paired_);
    auto* summary = new QWidget;
    summary->setFixedWidth(194);
    auto* summaryLayout = new QVBoxLayout(summary);
    summaryLayout->setContentsMargins(0, 0, 0, 0);
    summaryLayout->setSpacing(12);
    auto* previewTitle = new QLabel;
    bindTranslation(previewTitle, [previewTitle] { previewTitle->setText(tr("Edited photo preview")); });
    summaryLayout->addWidget(previewTitle);
    auto* image = new QLabel;
    image->setObjectName("photoPreview");
    image->setAlignment(Qt::AlignCenter);
    image->setFixedSize(194, 130);
    if (!preview.isNull()) image->setPixmap(QPixmap::fromImage(preview).scaled(image->size(), Qt::KeepAspectRatio,
                                                        Qt::SmoothTransformation));
    image->setWordWrap(true);
    if (preview.isNull()) {
        bindTranslation(image, [image] { image->setText(tr("Preview unavailable")); });
        if (!inputs_.isEmpty()) image->setToolTip(inputs_.first().previewError);
    } else {
        bindTranslation(image, [image] {
            image->setToolTip(tr("SDR preview of photo edits. Export color, size, sharpening and watermarks are not shown."));
        });
    }
    summaryLayout->addWidget(image);
    const auto detail = [summaryLayout](const char* name) {
        auto* caption = new QLabel;
        bindTranslation(caption, [caption, name] { caption->setText(tr(name)); });
        caption->setObjectName("muted");
        summaryLayout->addWidget(caption);
        auto* value = new QLabel;
        value->setWordWrap(true);
        value->setObjectName("parameterLabel");
        summaryLayout->addWidget(value);
        return value;
    };
    outputName_ = detail(QT_TR_NOOP("File name"));
    outputSize_ = detail(QT_TR_NOOP("Dimensions"));
    outputColor_ = detail(QT_TR_NOOP("Color"));
    auto* conflict = detail(QT_TR_NOOP("Existing files"));
    bindTranslation(conflict, [conflict] { conflict->setText(tr("Refuse and report a conflict")); });
    auto* note = new QLabel(tr("Exports run in the background.\nTrack progress in Jobs."));
    bindTranslation(note, [note] { note->setText(tr("Exports run in the background.\nTrack progress in Jobs.")); });
    note->setObjectName("muted");
    note->setWordWrap(true);
    summaryLayout->addWidget(note);
    summaryLayout->addStretch();
    body->addWidget(summary);
    root->addLayout(body);
    error_ = new QLabel;
    error_->setObjectName("exportError");
    error_->setWordWrap(true);
    error_->setStyleSheet("color:#e6aaa5;font-size:12px");
    error_->hide();
    root->addWidget(error_);
    auto* actions = new QHBoxLayout;
    auto* count = new QLabel(tr("%1 photos · originals are never overwritten").arg(std::max(1, int(inputs_.size()))));
    count->setObjectName("statusText");
    actions->addWidget(count, 1);
    auto* cancel = new QPushButton(tr("Cancel"));
    bindTranslation(cancel, [cancel] { cancel->setText(tr("Cancel")); });
    cancel->setObjectName("secondary");
    auto* submit = new QPushButton(windowTitle());
    submit->setObjectName("primary");
    bindTranslation(this, [this, title, submit, close, count, preset] {
        setWindowTitle(inputs_.size() > 1 ? tr("Export %1 photos").arg(inputs_.size()) : tr("Export photo"));
        title->setText(windowTitle());
        submit->setText(windowTitle());
        close->setAccessibleName(tr("Close export"));
        count->setText(tr("%1 photos · originals are never overwritten").arg(std::max(1, int(inputs_.size()))));
        const QSignalBlocker blocker(preset);
        preset->setItemText(0, tr("Web · sRGB JPEG"));
        preset->setItemText(1, tr("Full resolution · TIFF"));
        preset->setItemText(2, tr("HDR + SDR delivery"));
    });
    submit->setDefault(true);
    submit->setEnabled(!formats_.isEmpty());
    actions->addWidget(cancel);
    actions->addWidget(submit);
    root->addLayout(actions);
    connect(cancel, &QPushButton::clicked, this, &QDialog::reject);
    connect(submit, &QPushButton::clicked, this, [this] {
        const auto jobs = requests();
        const char* problem = nullptr;
        QString argument;
        if (jobs.isEmpty()) problem = QT_TR_NOOP("Choose an available export format.");
        else if (!QDir(destination_->text()).exists())
            problem = QT_TR_NOOP("Choose an existing destination folder.");
        else if (watermark_->isEnabled() && watermark_->currentIndex() == 1 && watermarkText_->text().trimmed().isEmpty())
            problem = QT_TR_NOOP("Enter watermark text or select None.");
        if (watermark_->isEnabled() && watermark_->currentIndex() == 2 && !QFileInfo(watermarkImage_->text()).isFile())
            problem = QT_TR_NOOP("Choose an existing watermark image.");
        for (const auto& job : jobs) {
            const auto path = job.options.value_or(ExportOptions{}).path;
            if (QFileInfo::exists(path)) {
                problem = QT_TR_NOOP("Destination already exists: %1");
                argument = QFileInfo(path).fileName();
            }
        }
        if (problem) {
            showError(problem, argument);
            adjustSize();
            return;
        }
        QSettings().setValue("export/destination", destination_->text());
        QSettings().setValue("export/lastSettings", QJsonDocument(settings()).toJson(QJsonDocument::Compact));
        accept();
    });
    for (auto* combo : {format_, profile_, size_})
        connect(combo, qOverload<int>(&QComboBox::currentIndexChanged), this,
                &ExportDialog::updatePreview);
    connect(name_, &QLineEdit::textChanged, this, &ExportDialog::updatePreview);
    connect(format_, qOverload<int>(&QComboBox::activated), this, [this](int index) {
        if (index >= 0) error_->hide();
    });
    connect(preset, qOverload<int>(&QComboBox::activated), this, [this, preset](int i) {
        if (i >= 3) { restoreSettings(preset->itemData(i).toJsonObject()); return; }
        const QString wanted = i == 1 ? "TIFF · 16-bit" : i == 2 ? "HDR PNG · PQ" : "JPEG";
        selectFormat(wanted);
        size_->setCurrentIndex(i == 0 ? 0 : 1);
        profile_->setCurrentIndex(0);
        paired_->setChecked(i == 2 && paired_->isEnabled());
    });
    restoreSettings(QJsonDocument::fromJson(QSettings().value("export/lastSettings").toByteArray()).object());
    bindTranslation(this, [this] {
        updatePreview();
        if (!errorSource_.isEmpty()) {
            auto message = tr(errorSource_.constData());
            if (message.contains("%1")) message = message.arg(errorArgument_);
            error_->setText(message);
        }
    });
    adjustSize();
}
void ExportDialog::showError(const char* source, const QString& argument) {
    errorSource_ = source;
    errorArgument_ = argument;
    auto message = tr(source);
    if (message.contains("%1")) message = message.arg(argument);
    error_->setText(message);
    error_->show();
}
void ExportDialog::selectFormat(const QString& name) {
    const int index = name.isEmpty() ? (formats_.isEmpty() ? -1 : 0) : format_->findText(name);
    format_->setCurrentIndex(index);
    if (index < 0) {
        showError(QT_TR_NOOP("The preset format %1 is unavailable for this build or selection. Choose another format."), name);
    } else error_->hide();
}
void ExportDialog::updatePreview() {
    const bool available = format_->currentIndex() >= 0 && format_->currentIndex() < formats_.size();
    if (auto* submit = findChild<QPushButton*>("primary")) submit->setEnabled(available);
    if (!available) {
        outputName_->clear(); outputSize_->clear(); outputColor_->clear();
        return;
    }
    const auto format = formats_[format_->currentIndex()];
    const auto label = format.name;
    const bool hdr = label.contains("HDR") || label == "OpenEXR", raw = label == "DNG";
    quality_->setEnabled(format.format.isObject() && !label.contains("TIFF"));
    profile_->setEnabled(!hdr && !raw);
    size_->setEnabled(!raw);
    longEdge_->setEnabled(!raw && size_->currentIndex() == 0);
    sharpen_->setEnabled(!raw);
    watermark_->setEnabled(!raw);
    paired_->setEnabled(hdr);
    if (!hdr)
        paired_->setChecked(false);
    const auto jobs = requests();
    QStringList names;
    for (int i = 0; i < std::min(4, int(jobs.size())); ++i)
        names.append(QFileInfo(jobs[i].options.value_or(ExportOptions{}).path).fileName());
    if (jobs.size() > 4) names.append(tr("…and %1 more files").arg(jobs.size() - 4));
    outputName_->setText(names.join('\n'));
    auto size = sourceSize_;
    if (size_->currentIndex() == 0 && !raw && size.width() > 0 && size.height() > 0)
        size.scale(longEdge_->value(), longEdge_->value(), Qt::KeepAspectRatio);
    outputSize_->setText(inputs_.size() > 1 ? tr("Size is calculated separately for each photo") : tr("%1 × %2").arg(size.width()).arg(size.height()));
    outputColor_->setText(raw   ? tr("Original RAW data")
                          : hdr ? tr("Rec.2020 · HDR")
                                : profile_->currentText());
}
QJsonObject ExportDialog::settings() const {
    return {{"format", format_->currentText()}, {"quality", quality_->value()},
        {"size", size_->currentIndex()}, {"longEdge", longEdge_->value()},
        {"profile", profile_->currentIndex()}, {"metadata", metadata_->currentIndex()},
        {"sharpen", sharpen_->currentIndex()}, {"name", name_->text()},
        {"artist", artist_->text()}, {"copyright", copyright_->text()},
        {"watermark", watermark_->currentIndex()}, {"text", watermarkText_->text()},
        {"image", watermarkImage_->text()}, {"wmSize", watermarkSize_->value()},
        {"opacity", watermarkOpacity_->value()}, {"margin", watermarkMargin_->value()},
        {"anchor", watermarkAnchor_->currentIndex()}, {"color", watermarkColor_.name()},
        {"paired", paired_->isChecked()}};
}
void ExportDialog::restoreSettings(const QJsonObject& value) {
    if (value.isEmpty()) return;
    selectFormat(value["format"].toString());
    quality_->setValue(value["quality"].toInt(90));
    longEdge_->setValue(value["longEdge"].toInt(2400));
    const auto combo = [&value](QComboBox* control, const char* key) {
        const int index = value[key].toInt();
        if (index >= 0 && index < control->count()) control->setCurrentIndex(index);
    };
    combo(size_, "size"); combo(profile_, "profile"); combo(metadata_, "metadata");
    combo(sharpen_, "sharpen"); combo(watermark_, "watermark"); combo(watermarkAnchor_, "anchor");
    name_->setText(value["name"].toString("{name}-edit"));
    artist_->setText(value["artist"].toString()); copyright_->setText(value["copyright"].toString());
    watermarkText_->setText(value["text"].toString()); watermarkImage_->setText(value["image"].toString());
    watermarkSize_->setValue(value["wmSize"].toDouble(4));
    watermarkOpacity_->setValue(value["opacity"].toDouble(60));
    watermarkMargin_->setValue(value["margin"].toDouble(3));
    const QColor color(value["color"].toString("#ffffff"));
    if (color.isValid()) watermarkColor_ = color;
    paired_->setChecked(value["paired"].toBool());
    updatePreview();
}
QString ExportDialog::outputStem(const QString& name, int sequence, int total) const {
    auto fallback = QFileInfo(name).completeBaseName();
    if (fallback.isEmpty()) fallback = "export";
    auto stem = QString(name_->text()).replace("{name}", fallback)
        .replace("{seq}", QString::number(sequence).rightJustified(QString::number(std::max(1, total)).size(), '0'));
    stem.replace('/', '_').replace('\\', '_').replace(QChar(0), '_');
    stem = stem.trimmed();
    if (stem.isEmpty() || stem == "." || stem == "..") stem = fallback;
    return stem;
}
QList<commands::Export> ExportDialog::requests() const {
    if (inputs_.isEmpty()) return requestsFor(source_, 1, 1);
    QList<commands::Export> result;
    QSet<QString> destinations;
    for (int i = 0; i < inputs_.size(); ++i) {
        const auto input = inputs_[i];
        for (const auto& value : requestsFor(input.name, i + 1, int(inputs_.size()))) {
            auto job = value;
            job.source_ref = input.source;
            job.params = input.params;
            job.source_name = input.name;
            auto options = job.options.value_or(ExportOptions{});
            const QFileInfo original(options.path);
            auto path = original.absoluteFilePath();
            int suffix = 2;
            while (destinations.contains(path))
                path = original.dir().absoluteFilePath(original.completeBaseName() + '-' + QString::number(suffix++) + '.' + original.suffix());
            destinations.insert(path);
            options.path = path;
            job.options = options;
            result.append(job);
        }
    }
    return result;
}
QList<commands::Export> ExportDialog::requestsFor(const QString& name, int sequence, int total) const {
    if (format_->currentIndex() < 0 || format_->currentIndex() >= formats_.size()) return {};
    const auto spec = formats_[format_->currentIndex()];
    auto format = spec.format;
    if (format.isObject()) {
        auto object = format.toObject();
        for (auto it = object.begin(); it != object.end(); ++it) {
            auto options = it.value().toObject();
            if (options.contains("quality")) {
                options["quality"] = quality_->value();
                it.value() = options;
            }
        }
        format = object;
    }
    const QStringList profiles{"Srgb", "DisplayP3", "AdobeRgb", "ProPhoto", "Rec2020"},
        metas{"CopyrightOnly", "All", "Strip"}, sharp{"Standard", "Off", "Low", "High"};
    const auto stem = outputStem(name, sequence, total);
    ExportOptions options;
    options.path = QDir(destination_->text()).absoluteFilePath(stem + '.' + spec.extension);
    options.format = format;
    options.longEdge = size_->isEnabled() && size_->currentIndex() == 0 ? std::optional<int>{longEdge_->value()} : std::optional<int>{};
    options.output = profiles[profile_->currentIndex()];
    options.metadata = metas[metadata_->currentIndex()];
    options.sharpening = sharp[sharpen_->currentIndex()];
    if (!artist_->text().isEmpty()) options.artist = artist_->text();
    if (!copyright_->text().isEmpty()) options.copyright = copyright_->text();
    if (watermark_->currentIndex() != 0 && watermark_->isEnabled())
        options.watermark = WatermarkOptions{
            watermark_->currentIndex() == 1 ? WatermarkOptions::Kind::Text : WatermarkOptions::Kind::Image,
            watermark_->currentIndex() == 1 ? watermarkText_->text() : watermarkImage_->text(),
            watermarkAnchor_->currentData().toString(), watermarkSize_->value() / 100,
            watermarkMargin_->value() / 100, watermarkOpacity_->value() / 100,
            {watermarkColor_.red(), watermarkColor_.green(), watermarkColor_.blue()}};
    QList<commands::Export> jobs{commands::Export{.options = options, .overwrite = false}};
    if (paired_->isEnabled() && paired_->isChecked()) {
        options.path = QDir(destination_->text()).absoluteFilePath(stem + "-sdr.jpg");
        options.format = QJsonObject{{"Jpeg", QJsonObject{{"quality", quality_->value()}}}};
        options.output = "Srgb";
        jobs.append(commands::Export{.options = options, .overwrite = false});
    }
    return jobs;
}
} // namespace aigle
