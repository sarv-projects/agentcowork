#!/usr/bin/env python3
"""Live E9 fixture: the tiny window the Linux E2E drives.

Title contains `agentcowork-e2e` (the test finds the window by that), shows a
`GO` button and a status label that flips to `CLICKED` when the button is
pressed — so the test can OCR before/after and verify through the real window.
"""
import tkinter as tk

root = tk.Tk()
root.title("agentcowork-e2e")
root.geometry("420x220")
root.configure(bg="white")

status = tk.Label(root, text="E9 OK", font=("Helvetica", 28), bg="white", fg="black")
status.pack(pady=24)


def on_go():
    status.config(text="CLICKED")


go = tk.Button(root, text="GO", font=("Helvetica", 24), width=8, command=on_go)
go.pack(pady=8)

root.after(60000, root.destroy)
root.mainloop()
